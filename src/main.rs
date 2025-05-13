use std::io::stdout;

use crossterm::{
    queue,
    style::{Attribute, Color, SetAttribute, SetBackgroundColor, SetForegroundColor},
};
use html_parser::{Dom, Node};
use reqwest::{Client, Response, Url};

const ERROR_PAGE_BODY: &str = include_str!("error.html");
const HOME_PAGE_BODY: &str = include_str!("home.html");

async fn parse_html_request(request: Result<Response, reqwest::Error>) -> Dom {
    if let Ok(response) = request {
        if let Ok(text) = response.text().await {
            if let Ok(dom) = Dom::parse(&text) {
                return dom;
            }
        }
    }
    Dom::parse(ERROR_PAGE_BODY).expect("error page should be valid html")
}

struct Webpage {
    body: Dom,
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
        let body = Dom::parse(body_text)
            .unwrap_or(Dom::parse(ERROR_PAGE_BODY).expect("error page should be valid html"));
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
    parent_interactable_element: Option<InteractableElement>,
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
            parent_interactable_element: None,
        }
    }
}
impl ProcessState {
    /// Uses crossterm to apply styles to console output.
    /// Used for color, italics, bold text, etc
    fn format_terminal(&self) {
        let mut foreground_color = self.foreground_color;
        let mut underlined = self.underlined;

        if self.parent_interactable_element.is_some() {
            underlined = true;
            foreground_color = Color::Green;
        }

        queue!(stdout(), SetAttribute(Attribute::Reset)).unwrap();
        queue!(stdout(), SetForegroundColor(foreground_color)).unwrap();
        queue!(stdout(), SetBackgroundColor(self.background_color)).unwrap();
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
/// Writes/stores all interactable elements in a buffer.
fn process_element(
    items: &Vec<Node>,
    interactables_buf: &mut Vec<(String, InteractableElement)>,
    recursion_level: usize,
    process_state: ProcessState,
    mut ended_with_newline: bool,
) {
    for item in items {
        match item {
            Node::Text(text) => {
                let mut text = text.clone();
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

                // if this text is part of an interactable element, add it to the interactables buffer
                if let Some(interactable) = &process_state.parent_interactable_element {
                    interactables_buf.push((text, interactable.clone()));
                }
            }
            Node::Element(element) => {
                let mut new_process_state = process_state.clone();
                if IGNORE_ELEMENTS.contains(&element.name.as_str()) {
                    continue;
                }
                let element_needs_linebreak =
                    ["p", "pre"].contains(&element.name.as_str()) || element.name.starts_with("h");

                if element_needs_linebreak && !ended_with_newline {
                    println!();
                }

                match element.name.as_str() {
                    "pre" => {
                        new_process_state.respect_whitespace = true;
                        new_process_state.background_color = Color::Black;
                    }
                    "em" => {
                        new_process_state.italics = true;
                    }
                    "a" => {
                        if let Some(path) = element.attributes.get("href") {
                            new_process_state.parent_interactable_element =
                                Some(InteractableElement::Link(path.clone().unwrap_or_default()));
                        }
                    }
                    _ => {
                        if element.name.starts_with("h") && element.name.len() == 2 {
                            new_process_state.foreground_color = Color::Red
                        }
                    }
                }

                process_element(
                    &element.children,
                    interactables_buf,
                    recursion_level + 1,
                    new_process_state.clone(),
                    ended_with_newline,
                );

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
}

struct Pop {
    client: Client,
    history: Vec<Webpage>,
}
impl Pop {
    async fn new() -> Self {
        let client = Client::new();
        let history = vec![Webpage::from_str(HOME_PAGE_BODY)];
        Self { client, history }
    }
    fn get_current_page(&self) -> &Webpage {
        self.history.last().expect("history should never be empty")
    }
    fn draw(&self) {
        let current_page = self.get_current_page();
        let mut interactable_elements = Vec::new();
        process_element(
            &current_page.body.children,
            &mut interactable_elements,
            0,
            ProcessState::default(),
            false,
        );
    }
}

#[tokio::main]
async fn main() {
    let pop = Pop::new().await;
    pop.draw();
}
