use std::{collections::HashMap, io::stdout, rc::Rc};

use crossterm::{
    event::{self, Event, KeyCode},
    queue,
    style::{Attribute, Color, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use reqwest::{Client, Response, Url};

use markup5ever_rcdom::{self as rcdom, Node, NodeData};

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

struct Webpage {
    body: RcDom,
    url: Option<Url>,
}
impl Webpage {
    async fn from_url(url: Url, client: &Client) -> Self {
        let body_request = client.get(url.clone()).send().await;
        let body = parse_html_request(body_request).await;
        Self {
            body,
            url: Some(url),
        }
    }
    fn from_str(body_text: &str) -> Self {
        let mut bytes = body_text.as_bytes();
        let dom = parse_document(RcDom::default(), ParseOpts::default())
            .from_utf8()
            .read_from(&mut bytes);
        let body = dom.unwrap_or(get_error_dom());
        Self { body, url: None }
    }
}

const IGNORE_ELEMENTS: &[&str] = &["style", "script", "title"];

fn trim_repeated_whitespace(s: &str) -> String {
    let words: Vec<_> = s.split_whitespace().collect();
    words.join(" ")
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
    fn format_terminal(&self) {
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

        queue!(stdout(), SetAttribute(Attribute::Reset)).unwrap();
        queue!(stdout(), SetForegroundColor(foreground_color)).unwrap();
        queue!(stdout(), SetBackgroundColor(background_color)).unwrap();
        if self.bold {
            queue!(stdout(), SetAttribute(Attribute::Bold)).unwrap();
        }
        if self.italics {
            queue!(stdout(), SetAttribute(Attribute::Italic)).unwrap();
        }
        if underlined {
            queue!(stdout(), SetAttribute(Attribute::Underlined)).unwrap();
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum InteractableElement {
    Link(String),
}

/// Recursively draws elements.
fn process_element(
    items: &Vec<Rc<Node>>,
    selection_index: Option<usize>,
    current_index: &mut usize,
    recursion_level: usize,
    process_state: ProcessState,
    mut ended_with_newline: bool,
) -> Option<InteractableElement> {
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
                print!(" ");
                process_state.format_terminal();
                print!("{}", text);
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
                    println!();
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

                            if let Some(selection_index) = selection_index {
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
                let result = process_element(
                    &item.children.clone().into_inner(),
                    selection_index,
                    current_index,
                    recursion_level + 1,
                    new_process_state.clone(),
                    ended_with_newline,
                );
                if result.is_some() {
                    return_value = result;
                }

                // restore old process state
                // i.e. the parent elements style
                if new_process_state != process_state {
                    process_state.format_terminal();
                }
                ended_with_newline = element_needs_linebreak;
                if element_needs_linebreak {
                    println!();
                }
            }
            _ => {}
        }
    }
    return_value
}

struct Pop {
    client: Client,
    history: Vec<Webpage>,
    selected_element: Option<InteractableElement>,
}
impl Pop {
    async fn new() -> Self {
        let client = Client::new();
        let history = vec![Webpage::from_str(HOME_PAGE_BODY)];
        Self {
            client,
            history,
            selected_element: None,
        }
    }
    fn get_current_page(&self) -> &Webpage {
        self.history.last().expect("history should never be empty")
    }
    fn draw(&mut self, selection_index: Option<usize>) {
        let current_page = self.get_current_page();
        queue!(stdout(), terminal::Clear(terminal::ClearType::All)).unwrap();
        self.selected_element = process_element(
            &current_page.body.document.children.clone().into_inner(),
            selection_index,
            &mut 0,
            0,
            ProcessState::default(),
            false,
        );
    }
    fn run(&mut self) {
        let mut selection_index = None;
        loop {
            self.draw(selection_index);
            let key = event::read().unwrap();
            if let Event::Key(key) = key {
                if key.is_press() {
                    match key.code {
                        KeyCode::Right => match &mut selection_index {
                            Some(value) => {
                                *value += 1;
                            }
                            None => {
                                selection_index = Some(0);
                            }
                        },
                        KeyCode::Left => match &mut selection_index {
                            Some(value) => {
                                *value = value.saturating_sub(1);
                            }
                            None => {
                                selection_index = Some(0);
                            }
                        },
                        _ => {}
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let mut pop = Pop::new().await;
    pop.run();
}
