use scraper::{ElementRef, Html, Selector};

/// Pick the element most likely to contain the primary article content.
///
/// Heuristic, in priority order: `<article>`, `<main>`, `[role=main]`,
/// then the largest `<div>` by text length, falling back to `<body>`.
pub fn content_root(doc: &Html) -> Option<ElementRef<'_>> {
    for sel in ["article", "main", "[role=main]"] {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                return Some(el);
            }
        }
    }

    // Fall back to the largest text-bearing <div>.
    if let Ok(div_sel) = Selector::parse("div") {
        let mut best: Option<(usize, ElementRef)> = None;
        for el in doc.select(&div_sel) {
            let len = el.text().map(|t| t.trim().len()).sum::<usize>();
            if best.as_ref().is_none_or(|(b, _)| len > *b) {
                best = Some((len, el));
            }
        }
        if let Some((len, el)) = best {
            if len > 0 {
                return Some(el);
            }
        }
    }

    Selector::parse("body")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
}

/// Extract the page title from `<title>` or the first `<h1>`.
pub fn extract_title(doc: &Html) -> String {
    for sel in ["title", "h1"] {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(el) = doc.select(&selector).next() {
                let t = el.text().collect::<String>().trim().to_string();
                if !t.is_empty() {
                    return t;
                }
            }
        }
    }
    String::new()
}
