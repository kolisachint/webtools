//! Markdown conversion. Unlike the text path, markdown keeps links inline as
//! `[text](url)` for maximum fidelity — the right trade-off when the consumer
//! wants a faithful, re-renderable document rather than minimal tokens.

use ego_tree::NodeRef;
use scraper::node::Node;
use scraper::Html;
use url::Url;

use crate::extract;

fn resolve(href: &str, base: &Option<Url>) -> String {
    match base {
        Some(b) => b
            .join(href)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| href.to_string()),
        None => href.to_string(),
    }
}

fn walk(node: NodeRef<Node>, out: &mut String, base: &Option<Url>) {
    match node.value() {
        Node::Text(t) => out.push_str(&t[..]),
        Node::Element(el) => {
            let name = el.name();
            if super::is_skippable(name) {
                return;
            }

            let prefix = match name {
                "h1" => Some("\n# "),
                "h2" => Some("\n## "),
                "h3" => Some("\n### "),
                "h4" => Some("\n#### "),
                "h5" => Some("\n##### "),
                "h6" => Some("\n###### "),
                "li" => Some("\n- "),
                "blockquote" => Some("\n> "),
                _ => None,
            };

            if name == "br" {
                out.push('\n');
                return;
            }

            if name == "a" {
                let mut inner = String::new();
                for child in node.children() {
                    walk(child, &mut inner, base);
                }
                let inner = inner.trim().to_string();
                match el.attr("href") {
                    Some(href) if !href.trim().is_empty() && !href.starts_with('#') => {
                        out.push_str(&format!("[{}]({})", inner, resolve(href, base)));
                    }
                    _ => out.push_str(&inner),
                }
                return;
            }

            if name == "code" {
                let mut inner = String::new();
                for child in node.children() {
                    walk(child, &mut inner, base);
                }
                out.push_str(&format!("`{}`", inner.trim()));
                return;
            }

            if let Some(p) = prefix {
                out.push_str(p);
            }
            for child in node.children() {
                walk(child, out, base);
            }
            if matches!(
                name,
                "p" | "div" | "section" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            ) {
                out.push('\n');
            }
        }
        _ => {}
    }
}

pub fn html_to_markdown(html: &str, base_url: &str) -> String {
    let doc = Html::parse_document(html);
    let root = match extract::content_root(&doc) {
        Some(el) => el,
        None => return String::new(),
    };
    let base = Url::parse(base_url).ok();
    let mut out = String::new();
    for child in root.children() {
        walk(child, &mut out, &base);
    }
    out
}
