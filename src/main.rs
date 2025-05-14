use std::{
    collections::HashMap,
    io::{stdin, stdout},
    rc::Rc,
};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    execute, queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use markup5ever_rcdom::{self as rcdom, Node, NodeData};
use reqwest::{Client, Response, Url};
use std::io::Write;

use html5ever::driver::ParseOpts;
use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use rcdom::RcDom;

const ERROR_PAGE_BODY: &str = include_str!("error.html");
const HOME_PAGE_BODY: &str = include_str!("home.html");

fn get_error_dom() -> RcDom {
    let mut bytes = ERROR_PAGE_BODY.as_bytes();

    parse_document(RcDom::default(), ParseOpts::default())
        .from_utf8()
        .read_from(&mut bytes)
        .expect("error page should be valid html")
}

async fn parse_html_request(request: Result<Response, reqwest::Error>) -> RcDom {
    if let Ok(response) = request {
        if let Ok(text) = response.text().await {
            let mut bytes = text.as_bytes();
            let dom = parse_document(RcDom::default(), ParseOpts::default().clone())
                .from_utf8()
                .read_from(&mut bytes);
            if let Ok(dom) = dom {
                return dom;
            }
        }
    }
    get_error_dom()
}

#[derive(Clone, PartialEq, Eq)]
struct ProcessState {
    respect_whitespace: bool,
    foreground_color: Color,
    background_color: Color,
    bold: bool,
    italics: bool,
    underlined: bool,
    interactable_element: Option<InteractableElement>,
    currently_selected: bool,
}
impl Default for ProcessState {
    fn default() -> Self {
        Self {
            respect_whitespace: false,
            foreground_color: Color::White,
            background_color: Color::Reset,
            bold: false,
            italics: false,
            underlined: false,
            interactable_element: None,
            currently_selected: false,
        }
    }
}
impl ProcessState {
    /// Uses crossterm to apply styles to console output.
    /// Used for color, italics, bold text, etc
    fn format_terminal<T>(&self, buf: &mut T)
    where
        T: Write,
    {
        let mut foreground_color = self.foreground_color;
        let mut background_color = self.background_color;
        let mut underlined = self.underlined;

        if self.interactable_element.is_some() {
            underlined = true;
            foreground_color = Color::Cyan;
        }
        if self.currently_selected {
            background_color = Color::White;
        }

        queue!(buf, SetAttribute(Attribute::Reset)).unwrap();
        queue!(buf, SetForegroundColor(foreground_color)).unwrap();
        queue!(buf, SetBackgroundColor(background_color)).unwrap();
        if self.bold {
            queue!(buf, SetAttribute(Attribute::Bold)).unwrap();
        }
        if self.italics {
            queue!(buf, SetAttribute(Attribute::Italic)).unwrap();
        }
        if underlined {
            queue!(buf, SetAttribute(Attribute::Underlined)).unwrap();
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum InteractableElement {
    Link(String),
}

const IGNORE_ELEMENTS: &[&str] = &["style", "script", "title"];

fn trim_repeated_whitespace(s: &str) -> String {
    let words: Vec<_> = s.split_whitespace().collect();
    words.join(" ")
}

struct Webpage {
    body: RcDom,
    url: Option<Url>,
    title: Option<String>,
    selection_index: Option<usize>,
    scroll: usize,
}
impl Webpage {
    async fn from_url(url: Url, client: &Client) -> Self {
        let body_request = client.get(url.clone()).send().await;
        let body = parse_html_request(body_request).await;
        Self {
            body,
            url: Some(url),
            title: None,
            selection_index: None,
            scroll: 0,
        }
    }
    fn from_str(body_text: &str) -> Self {
        let mut bytes = body_text.as_bytes();
        let dom = parse_document(RcDom::default(), ParseOpts::default())
            .from_utf8()
            .read_from(&mut bytes);
        let body = dom.unwrap_or(get_error_dom());
        Self {
            body,
            url: None,
            title: None,
            selection_index: None,
            scroll: 0,
        }
    }

    // Draws entire page to buffer
    fn process_page<T>(&mut self, buf: &mut T) -> Option<InteractableElement>
    where
        T: Write,
    {
        self.process_element(
            buf,
            &self.body.document.children.clone().into_inner(),
            &mut 0,
            ProcessState::default(),
            false,
        )
    }

    /// Recursively draws elements.
    fn process_element<T>(
        &mut self,
        buf: &mut T,
        items: &Vec<Rc<Node>>,
        current_index: &mut usize,
        process_state: ProcessState,
        mut ended_with_newline: bool,
    ) -> Option<InteractableElement>
    where
        T: Write,
    {
        let mut return_value = None;
        for item in items {
            match &item.data {
                NodeData::Text { contents } => {
                    let mut text = contents.borrow().to_string();

                    if !process_state.respect_whitespace {
                        text = text.replace("\n", "");
                        text = text.replace("\r", "");
                        text = strip_ansi_escapes::strip_str(text);
                        text = trim_repeated_whitespace(&text);
                    }
                    if !text.is_empty() {
                        write!(buf, " ").unwrap();
                        process_state.format_terminal(buf);
                        write!(buf, "{}", text).unwrap();
                    }
                    ended_with_newline = false;
                }
                NodeData::Element {
                    name,
                    attrs,
                    template_contents: _,
                    mathml_annotation_xml_integration_point: _,
                } => {
                    let name = name.local.to_string();
                    let mut new_process_state = process_state.clone();
                    if IGNORE_ELEMENTS.contains(&name.as_str()) {
                        continue;
                    }
                    let element_needs_linebreak =
                        ["p", "pre"].contains(&name.as_str()) || name.starts_with("h");

                    if element_needs_linebreak && !ended_with_newline {
                        writeln!(buf,).unwrap();
                    }
                    let mut attributes_map = HashMap::new();
                    for a in attrs.borrow().iter() {
                        attributes_map.insert(a.name.local.to_string(), a.value.to_string());
                    }

                    match name.as_str() {
                        "pre" => {
                            new_process_state.respect_whitespace = true;
                            new_process_state.background_color = Color::Black;
                        }
                        "em" => {
                            new_process_state.italics = true;
                        }
                        "a" => {
                            if let Some(path) = attributes_map.get("href") {
                                let interactable_element = InteractableElement::Link(path.clone());

                                new_process_state.interactable_element =
                                    Some(interactable_element.clone());

                                if let Some(selection_index) = self.selection_index {
                                    if *current_index == selection_index {
                                        new_process_state.currently_selected = true;
                                        return_value = Some(interactable_element);
                                    }
                                    *current_index += 1;
                                }
                            }
                        }
                        _ => {
                            if name.starts_with("h") && name.len() == 2 {
                                new_process_state.foreground_color = Color::Red
                            }
                        }
                    }
                    let result = self.process_element(
                        buf,
                        &item.children.clone().into_inner(),
                        current_index,
                        new_process_state.clone(),
                        ended_with_newline,
                    );
                    if result.is_some() {
                        return_value = result;
                    }

                    // restore old process state
                    // i.e. the parent elements style
                    if new_process_state != process_state {
                        process_state.format_terminal(buf);
                    }
                    ended_with_newline = element_needs_linebreak;
                    if element_needs_linebreak {
                        writeln!(buf,).unwrap();
                    }
                }
                _ => {}
            }
        }
        return_value
    }
}
static POP_HEADER: &str = "POP    [Q]uit [G]oto page";

struct Pop {
    client: Client,
    history: Vec<Webpage>,
    selected_interactable: Option<InteractableElement>,
}
impl Pop {
    async fn new() -> Self {
        let user_agent = format!("Pop/{}", env!("CARGO_PKG_VERSION"));
        let client = Client::builder()
            .user_agent(user_agent)
            .build()
            .expect("network client should be constructable");
        let history = vec![Webpage::from_str(HOME_PAGE_BODY)];
        Self {
            client,
            history,
            selected_interactable: None,
        }
    }
    fn get_current_page(&mut self) -> &mut Webpage {
        self.history
            .last_mut()
            .expect("history should never be empty")
    }
    fn draw_current_page(&mut self) {
        let (screen_width, screen_height) = terminal::size().unwrap();
        let current_page = self.get_current_page();

        // render webpage to buffer
        let mut buf: Vec<u8> = Vec::new();
        let selected_interactable = current_page.process_page(&mut buf);

        // split buffer to each line
        let mut lines = buf.split(|f| *f == b'\n');

        // draw only the scrolled view
        let max_line = screen_height as isize;

        let scroll = current_page.scroll;

        let mut line_index = scroll as isize * -1;

        queue!(
            stdout(),
            cursor::MoveTo(0, 1),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )
        .unwrap();

        let skip = (line_index * -1).max(0) as usize;

        let mut in_start = true;
        while let Some(line) = lines.next() {
            // strip leading empty lines
            if in_start {
                if !line.is_empty() {
                    // when we reach first non empty line
                    in_start = false;
                    if skip > 0 {
                        for _ in 0..skip - 1 {
                            lines.next();
                        }
                        continue;
                    }
                } else {
                    continue;
                }
            }
            if line_index >= max_line {
                break;
            }
            // draw line and break when width is >= screen_width
            let mut buf: Vec<u8> = Vec::new();
            for byte in line {
                let mut new = buf.clone();
                new.push(*byte);
                let new_buf_width = String::from_utf8_lossy(&strip_ansi_escapes::strip(new))
                    .chars()
                    .count();
                if new_buf_width >= screen_width as usize {
                    stdout().lock().write_all(&buf).unwrap();
                    buf = Vec::new();
                    queue!(stdout(), cursor::MoveToNextLine(1)).unwrap();
                    line_index += 1;
                }
                buf.push(*byte);
            }
            stdout().lock().write_all(&buf).unwrap();
            queue!(stdout(), cursor::MoveToNextLine(1)).unwrap();
            line_index += 1;
        }

        queue!(
            stdout(),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )
        .unwrap();

        self.selected_interactable = selected_interactable;
    }
    fn draw_navbar(&self) {
        // draw navbar
        queue!(
            stdout(),
            cursor::MoveTo(0, 0),
            SetBackgroundColor(Color::White),
            SetForegroundColor(Color::Black)
        )
        .unwrap();
        stdout().lock().write_all(POP_HEADER.as_bytes()).unwrap();
        queue!(stdout(), ResetColor).unwrap();
    }
    async fn run(&mut self) {
        queue!(
            stdout(),
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All),
        )
        .unwrap();

        loop {
            self.draw_current_page();
            self.draw_navbar();
            stdout().flush().unwrap();
            let key = event::read().unwrap();
            if let Event::Key(key) = key {
                if !key.is_press() {
                    continue;
                }
                let page: &mut Webpage = self.get_current_page();
                let page_url = page.url.clone();
                let selection_index = &mut page.selection_index;
                match key.code {
                    KeyCode::Right => match selection_index {
                        Some(value) => {
                            *value += 1;
                        }
                        None => {
                            *selection_index = Some(0);
                        }
                    },
                    KeyCode::Left => match selection_index {
                        Some(value) => {
                            *value = value.saturating_sub(1);
                        }
                        None => {
                            *selection_index = Some(0);
                        }
                    },
                    KeyCode::Down => page.scroll += 1,
                    KeyCode::Up => page.scroll = page.scroll.saturating_sub(1),
                    KeyCode::Enter => {
                        if let Some(selected_interactable) = &self.selected_interactable {
                            match selected_interactable {
                                InteractableElement::Link(link) => {
                                    let current_url = page_url;
                                    let options = Url::options().base_url(current_url.as_ref());
                                    match options.parse(link) {
                                        Ok(url) => {
                                            let webpage =
                                                Webpage::from_url(url, &self.client).await;
                                            self.history.push(webpage);
                                        }
                                        Err(e) => {
                                            queue!(
                                                stdout(),
                                                cursor::MoveTo(POP_HEADER.len() as u16 + 2, 0),
                                                terminal::Clear(terminal::ClearType::UntilNewLine)
                                            )
                                            .unwrap();
                                            print!("error: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Char(char) => match char {
                        'q' => {
                            break;
                        }
                        'g' => {
                            execute!(
                                stdout(),
                                cursor::Show,
                                cursor::MoveTo(POP_HEADER.len() as u16 + 2, 0),
                                terminal::Clear(terminal::ClearType::UntilNewLine)
                            )
                            .unwrap();
                            let mut buf = String::new();
                            stdin().read_line(&mut buf).unwrap();
                            queue!(stdout(), cursor::Hide).unwrap();
                            match Url::parse(&buf) {
                                Ok(url) => {
                                    let webpage = Webpage::from_url(url, &self.client).await;
                                    self.history.push(webpage);
                                }
                                Err(e) => {
                                    queue!(
                                        stdout(),
                                        cursor::MoveTo(POP_HEADER.len() as u16 + 2, 0),
                                        terminal::Clear(terminal::ClearType::UntilNewLine)
                                    )
                                    .unwrap();
                                    print!("error: {}", e);
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }

        queue!(
            stdout(),
            cursor::Show,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        )
        .unwrap();
    }
}

#[tokio::main]
async fn main() {
    let mut pop = Pop::new().await;
    pop.run().await;
}
