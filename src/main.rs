use html_parser::{Dom, Node};
use reqwest::{Client, Response, Url};

const ERROR_PAGE_BODY: &str = include_str!("error.html");

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
}

const IGNORE_ELEMENTS: &[&str] = &["style", "script", "title"];

fn get_viewable_elements(items: &Vec<Node>, buf: &mut Vec<String>) {
    for item in items {
        match item {
            Node::Text(text) => {
                buf.push(text.clone());
            }
            Node::Element(element) => {
                if IGNORE_ELEMENTS.contains(&element.name.as_str()) {
                    continue;
                }
                get_viewable_elements(&element.children, buf);
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
        let history = vec![
            Webpage::from_url(
                Url::parse("https://example.com").expect("default url should be valid"),
                &client,
            )
            .await,
        ];
        Self { client, history }
    }
    fn get_current_page(&self) -> &Webpage {
        self.history.last().expect("history should never be empty")
    }
    fn draw(&self) {
        let current_page = self.get_current_page();
        let mut all_elements = Vec::new();
        get_viewable_elements(&current_page.body.children, &mut all_elements);
        for element in all_elements {
            println!("{element}");
        }
    }
}

#[tokio::main]
async fn main() {
    let pop = Pop::new().await;
    pop.draw();
}
