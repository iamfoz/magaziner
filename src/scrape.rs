//! Shared HTML-scraping helpers used by the magazine adapters.

use crate::validation::absolutize;
use scraper::{ElementRef, Html, Selector};

/// Pick the largest-width URL from an element's `data-srcset`/`srcset`, resolved against
/// `base`. A srcset entry is "url 1600w" (or "url 2x"); we parse the numeric descriptor
/// and keep the biggest.
pub fn best_from_srcset(img: &ElementRef<'_>, base: &str) -> Option<String> {
    let raw = img
        .value()
        .attr("data-srcset")
        .or_else(|| img.value().attr("srcset"))?;

    let mut best: Option<(u32, String)> = None;
    for candidate in raw.split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(url) = parts.next() else {
            continue;
        };
        let width = parts
            .next()
            .map(|d| d.trim_end_matches(['w', 'x']))
            .and_then(|d| d.parse::<u32>().ok())
            .unwrap_or(0);
        if best.as_ref().map(|(w, _)| width >= *w).unwrap_or(true) {
            best = Some((width, url.to_string()));
        }
    }
    best.map(|(_, url)| absolutize(base, &url))
}

/// The `content` attribute of the first element matching `selector`, if non-empty.
pub fn meta_content(doc: &Html, selector: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    doc.select(&sel)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
