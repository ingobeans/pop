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
/// Helper function to draw text to the screen by a coordinate
fn set_terminal_line(text: &str, x: usize, y: usize, overwrite: bool) -> std::io::Result<()> {
    if overwrite {
        queue!(
            stdout(),
            cursor::MoveTo(x as u16, y as u16),
            terminal::Clear(terminal::ClearType::CurrentLine)
        )?;
        print!("{text}");
    } else {
        queue!(stdout(), cursor::MoveTo(x as u16, y as u16))?;
        print!("{text}");
    }
    Ok(())
}

struct ProcessResult {
    tokens: Vec<Token>,
    selected_element: Option<InteractableElement>,
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
    fn process_page(&mut self) -> ProcessResult {
        let mut buf: Vec<Token> = Vec::new();
        let selected = self.process_element(
            &mut buf,
            &self.body.document.children.clone().into_inner(),
            &mut 0,
            ProcessState::default(),
            false,
        );
        ProcessResult {
            tokens: buf,
            selected_element: selected,
        }
    }

    /// Recursively draws elements.
    fn process_element(
        &mut self,
        buf: &mut Vec<Token>,
        items: &Vec<Rc<Node>>,
        current_index: &mut usize,
        process_state: ProcessState,
        mut ended_with_newline: bool,
    ) -> Option<InteractableElement> {
        let mut return_value = None;
        for item in items {
            match &item.data {
                NodeData::Text { contents } => {
                    let mut text = contents.borrow().to_string();

                    text = text.replace("\n", "");
                    text = text.replace("\r", "");
                    if !process_state.respect_whitespace {
                        text = trim_repeated_whitespace(&text);
                    }
                    if !text.is_empty() {
                        buf.push(Token::Padding);
                        buf.push(Token::Formatting(process_state.clone()));
                        buf.push(Token::Text(text));
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
                        buf.push(Token::Newline);
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
                        buf.push(Token::Formatting(process_state.clone()));
                    }
                    ended_with_newline = element_needs_linebreak;
                    if element_needs_linebreak {
                        buf.push(Token::Newline);
                    }
                }
                _ => {}
            }
        }
        return_value
    }
}
static POP_HEADER: &str = "POP    [Q]uit [G]oto page";

enum Token {
    Text(String),
    Formatting(ProcessState),
    Padding,
    Newline,
}

struct Pop {
    client: Client,
    history: Vec<Webpage>,
    selected_element: Option<InteractableElement>,
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
            selected_element: None,
        }
    }
    fn get_current_page(&mut self) -> &mut Webpage {
        self.history
            .last_mut()
            .expect("history should never be empty")
    }
    fn draw_current_page<T>(&mut self, mut stdout: T)
    where
        T: Write,
    {
        let (screen_width, screen_height) = terminal::size().unwrap();
        let (screen_width, screen_height) = (screen_width as usize, screen_height as usize);
        let current_page = self.get_current_page();

        // process webpage
        let result = current_page.process_page();
        let last_process_state = ProcessState::default();

        let mut row_index = 1;
        let mut column_index = 0;
        queue!(
            stdout,
            cursor::MoveTo(column_index as u16, row_index as u16),
            terminal::Clear(terminal::ClearType::FromCursorDown)
        )
        .unwrap();

        let mut scroll = current_page.scroll;
        let mut last_was_newline = true;

        for token in result.tokens {
            match &token {
                Token::Text(text) => {
                    let text_length = text.chars().count();
                    if text_length + column_index > screen_width {
                        column_index = 0;
                        if scroll == 0 {
                            row_index += 1;
                            if row_index >= screen_height {
                                break;
                            }
                        } else {
                            scroll -= 1;
                        }
                    }
                    if scroll == 0 {
                        set_terminal_line(&text, column_index, row_index, false).unwrap();
                    }
                    column_index += text_length;
                }
                Token::Newline => {
                    if last_was_newline {
                        continue;
                    }
                    if scroll == 0 {
                        row_index += 1;
                        if row_index >= screen_height {
                            break;
                        }
                    } else {
                        scroll -= 1;
                    }
                    column_index = 0;
                }
                Token::Formatting(state) => {
                    state.format_terminal(&mut stdout);
                    if last_process_state != *state {}
                }
                Token::Padding => {
                    if scroll == 0 {
                        set_terminal_line(" ", column_index, row_index, false).unwrap();
                        column_index += 1;
                    }
                }
            }
            last_was_newline = matches!(token, Token::Newline);
        }

        self.selected_element = result.selected_element;
    }
    fn draw_navbar<T>(&self, mut stdout: T)
    where
        T: Write,
    {
        // draw navbar
        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            SetBackgroundColor(Color::White),
            SetForegroundColor(Color::Black)
        )
        .unwrap();
        stdout.write_all(POP_HEADER.as_bytes()).unwrap();
        queue!(stdout, ResetColor).unwrap();
    }
    async fn run(&mut self) {
        queue!(
            stdout(),
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All),
        )
        .unwrap();

        let mut stdout = stdout().lock();
        loop {
            self.draw_current_page(&mut stdout);
            self.draw_navbar(&mut stdout);
            stdout.flush().unwrap();
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
                        if let Some(selected_interactable) = &self.selected_element {
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
                                                stdout,
                                                cursor::MoveTo(POP_HEADER.len() as u16 + 2, 0),
                                                terminal::Clear(terminal::ClearType::UntilNewLine)
                                            )
                                            .unwrap();
                                            write!(stdout, "error: {}", e).unwrap();
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
                                stdout,
                                cursor::Show,
                                cursor::MoveTo(POP_HEADER.len() as u16 + 2, 0),
                                terminal::Clear(terminal::ClearType::UntilNewLine)
                            )
                            .unwrap();
                            let mut buf = String::new();
                            stdin().read_line(&mut buf).unwrap();
                            queue!(stdout, cursor::Hide).unwrap();
                            match Url::parse(&buf) {
                                Ok(url) => {
                                    let webpage = Webpage::from_url(url, &self.client).await;
                                    self.history.push(webpage);
                                }
                                Err(e) => {
                                    queue!(
                                        stdout,
                                        cursor::MoveTo(POP_HEADER.len() as u16 + 2, 0),
                                        terminal::Clear(terminal::ClearType::UntilNewLine)
                                    )
                                    .unwrap();
                                    write!(stdout, "error: {}", e).unwrap();
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
            stdout,
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
