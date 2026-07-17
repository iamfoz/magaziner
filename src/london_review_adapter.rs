use crate::adapter::{ArticleData, IssueData, MagazineAdapter};
use crate::progress::Progress;
use crate::validation::{absolutize, is_valid_http_url};
use regex::Regex;
use scraper::{Html, Selector};

const BASE: &str = "https://www.lrb.co.uk";

pub struct LondonReviewAdapter;

impl MagazineAdapter for LondonReviewAdapter {
    fn extract_issue(&self, doc: &Html, progress: &Progress) -> IssueData {
        let articles_selector = Selector::parse("a.toc-item").unwrap();
        let title_selector = Selector::parse("title").unwrap();

        let links: Vec<String> = doc
            .select(&articles_selector)
            .filter_map(|el| el.value().attr("href"))
            .map(|s| absolutize(BASE, s))
            .collect();

        let title = doc
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>())
            .map(|t| clean_issue_title(&t))
            .unwrap_or_else(|| "Untitled".into());

        let cover_image_uri = extract_cover_uri(doc);

        if cover_image_uri.is_empty() {
            progress.warn("No cover image found on the issue page.");
        }
        progress.verbose(&format!("Found {} article links", links.len()));
        progress.verbose(&format!("Issue title: {}", title));
        progress.verbose(&format!("Cover image: {}", cover_image_uri));

        IssueData {
            links,
            title,
            cover_image_uri,
            publication_name: "London Review of Books".to_string(),
        }
    }

    fn extract_article(&self, doc: &Html, progress: &Progress) -> ArticleData {
        let title_selector = Selector::parse("title").unwrap();
        let byline_selector = Selector::parse("a[href^=\"/contributors/\"]").unwrap();
        let reviewed_items_selector = Selector::parse("div.reviewed-items").unwrap();
        let body_selector = Selector::parse("div.article-copy").unwrap();

        let title = doc
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>())
            .map(|t| clean_article_title(&t))
            .unwrap_or_else(|| "Untitled".into());

        let byline = doc
            .select(&byline_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty());

        let reviewed_items = doc
            .select(&reviewed_items_selector)
            .map(|el| el.inner_html())
            .collect::<Vec<_>>()
            .join("\n\n");

        let body = doc
            .select(&body_selector)
            .map(|el| el.inner_html())
            .collect::<Vec<_>>()
            .join("\n\n");

        let reviewed_block = if reviewed_items.trim().is_empty() {
            String::new()
        } else {
            format!("<div class=\"reviewed-items\">{}</div>", reviewed_items)
        };
        let complete_article = format!("{reviewed_block}{body}");

        progress.verbose(&format!("Extracted: {}", title));

        ArticleData {
            title,
            byline,
            body: complete_article,
        }
    }

    fn discover_issues(&self, doc: &Html, progress: &Progress) -> Vec<String> {
        let a_sel = Selector::parse("a").unwrap();
        // Canonical issue-index links only: /the-paper/vNN/nNN (no deeper path).
        let re = Regex::new(r"^(?:https://www\.lrb\.co\.uk)?/the-paper/v\d{2}/n\d{2}/?$").unwrap();

        let mut seen = std::collections::HashSet::new();
        let mut issues = Vec::new();
        for el in doc.select(&a_sel) {
            if let Some(href) = el.value().attr("href")
                && re.is_match(href.trim())
            {
                let url = absolutize(BASE, href.trim());
                let url = url.trim_end_matches('/').to_string();
                if seen.insert(url.clone()) {
                    issues.push(url);
                }
            }
        }
        progress.verbose(&format!("Discovered {} issue links", issues.len()));
        issues
    }
}

/// Extract the issue cover image URL, resolved to an absolute URL.
///
/// LRB markup has changed over time and cover images are lazy-loaded, so this tries a
/// series of strategies in order of preference:
///   1. A dedicated cover image element (several known container classes).
///   2. The Open Graph `og:image` meta tag — on an LRB issue page this is the cover,
///      and it's an absolute URL. This is the reliable fallback when the markup shifts.
///   3. The Twitter card image.
fn extract_cover_uri(doc: &Html) -> String {
    const COVER_SELECTORS: &[&str] = &[
        "div.article-issue-cover-image img",
        ".toc-cover img",
        "figure.issue-cover img",
        ".issue-cover img",
        "img.cover-image",
    ];

    for sel in COVER_SELECTORS {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        let Some(img) = doc.select(&selector).next() else {
            continue;
        };

        // 1. Prefer the highest-resolution candidate from the responsive srcset. On LRB
        //    this yields the full 2000-wide cover; the single-URL `data-appsrc` is a
        //    JS placeholder ("//images/...") and `og:image` is a cropped social card.
        if let Some(u) = best_from_srcset(&img)
            && is_valid_http_url(&u)
        {
            return u;
        }
        // 2. Fall back to single-URL attributes on the same element.
        if let Some(raw) = img
            .value()
            .attr("data-appsrc")
            .or_else(|| img.value().attr("data-src"))
            .or_else(|| img.value().attr("src"))
        {
            let abs = absolutize(BASE, raw.trim());
            if is_valid_http_url(&abs) {
                return abs;
            }
        }
    }

    // 3. Last resort: the social-image meta tag, rewritten from the cropped
    //    `social_image_on_bg` variant to the full-size cover.
    const META_SELECTORS: &[&str] = &[
        r#"meta[property="og:image"]"#,
        r#"meta[name="twitter:image"]"#,
    ];
    for sel in META_SELECTORS {
        if let Some(content) = meta_content(doc, sel) {
            let full = content.replace("/social_image_on_bg/", "/2000_filter/");
            let abs = absolutize(BASE, &full);
            if is_valid_http_url(&abs) {
                return abs;
            }
        }
    }

    String::new()
}

/// Pick the largest-width URL from an element's `data-srcset`/`srcset`. A srcset entry is
/// "url 1600w" (or "url 2x"); we parse the numeric descriptor and keep the biggest.
fn best_from_srcset(img: &scraper::ElementRef<'_>) -> Option<String> {
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
    best.map(|(_, url)| absolutize(BASE, &url))
}

/// The `content` attribute of the first element matching `selector`, if non-empty.
fn meta_content(doc: &Html, selector: &str) -> Option<String> {
    let sel = Selector::parse(selector).ok()?;
    doc.select(&sel)
        .next()
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// "Contents · Vol. 99 No. 3 · 15 March 2025" → "Vol. 99 No. 3 · 15 March 2025".
fn clean_issue_title(raw: &str) -> String {
    let t = raw.trim();
    match t.find("Vol.") {
        Some(idx) => t[idx..].trim().to_string(),
        None => t.to_string(),
    }
}

/// Strip the trailing site/date suffix from an article `<title>`, e.g.
/// "Headline · LRB 15 March 2025" → "Headline"; "Headline · London Review of Books" → "Headline".
fn clean_article_title(raw: &str) -> String {
    let parts: Vec<&str> = raw.trim().split(" · ").collect();
    let kept: Vec<&str> = parts
        .into_iter()
        .filter(|seg| {
            let s = seg.trim();
            !(s.starts_with("LRB") || s.contains("London Review of Books"))
        })
        .collect();
    let joined = kept.join(" · ");
    if joined.trim().is_empty() {
        raw.trim().to_string()
    } else {
        joined.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::Verbosity;
    use scraper::Html;
    use std::fs;

    fn load_html_fixture(path: &str) -> Html {
        let html = fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("Failed to read fixture at {}", path));
        Html::parse_document(&html)
    }

    #[test]
    fn test_extract_article_links_from_issue() {
        let doc = load_html_fixture("src/test/lrb/issue.html");
        let progress = Progress::new(Verbosity::Quiet);
        let adapter = LondonReviewAdapter;
        let issue = adapter.extract_issue(&doc, &progress);

        assert!(
            !issue.links.is_empty(),
            "Expected at least one article link"
        );
        assert_eq!(issue.title, "Vol. 99 No. 3 · 15 March 2025");
        assert!(
            issue
                .links
                .iter()
                .all(|l| l.starts_with("https://www.lrb.co.uk")),
            "All links should be absolute LRB URLs"
        );
        assert!(!issue.cover_image_uri.is_empty());
    }

    #[test]
    fn test_cover_uri_picks_largest_srcset() {
        let doc = load_html_fixture("src/test/lrb/issue.html");
        let progress = Progress::new(Verbosity::Quiet);
        let issue = LondonReviewAdapter.extract_issue(&doc, &progress);
        // The srcset has 400w and 1600w candidates; the largest (2000_filter) must win,
        // not the broken `//images/...` data-appsrc placeholder.
        assert_eq!(
            issue.cover_image_uri,
            "https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/cover.jpg"
        );
    }

    #[test]
    fn test_cover_real_lrb_markup() {
        // The exact structure from a live LRB issue page: broken //images placeholder in
        // data-appsrc, real absolute URLs in data-srcset, cropped social card in og:image.
        let html = r#"<html><head>
            <meta property="og:image" content="https://www.lrb.co.uk/storage/social_image_on_bg/images/4/0/1/9/30999104-1-eng-GB/478.jpg">
        </head><body>
          <div class="article-issue-cover-image"><span class="lrb-imageHolder">
            <img src="" data-appsrc="//images/4/0/1/9/30999104-1-eng-GB/478.jpg"
                 data-srcset="https://www.lrb.co.uk/storage/400_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg 400w, https://www.lrb.co.uk/storage/800_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg 800w, https://www.lrb.co.uk/storage/1200_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg 1200w, https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg 1600w"
                 class="lazyload" alt="">
          </span></div>
        </body></html>"#;
        let doc = Html::parse_document(html);
        assert_eq!(
            extract_cover_uri(&doc),
            "https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg"
        );
    }

    #[test]
    fn test_cover_og_image_social_variant_rewritten() {
        // If only og:image is available, rewrite the cropped social variant to full size.
        let html = r#"<html><head>
            <meta property="og:image" content="https://www.lrb.co.uk/storage/social_image_on_bg/images/4/0/1/9/x.jpg">
        </head><body></body></html>"#;
        let doc = Html::parse_document(html);
        assert_eq!(
            extract_cover_uri(&doc),
            "https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/x.jpg"
        );
    }

    #[test]
    fn test_cover_falls_back_to_og_image() {
        // No dedicated cover element; must fall back to the og:image meta tag.
        let html = r#"<html><head>
            <meta property="og:image" content="https://www.lrb.co.uk/storage/covers/n01.jpg">
        </head><body><a class="toc-item" href="/the-paper/v48/n01/a">x</a></body></html>"#;
        let doc = Html::parse_document(html);
        assert_eq!(
            extract_cover_uri(&doc),
            "https://www.lrb.co.uk/storage/covers/n01.jpg"
        );
    }

    #[test]
    fn test_cover_og_image_relative_is_absolutized() {
        let html = r#"<html><head>
            <meta property="og:image" content="/storage/covers/n01.jpg">
        </head><body></body></html>"#;
        let doc = Html::parse_document(html);
        assert_eq!(
            extract_cover_uri(&doc),
            "https://www.lrb.co.uk/storage/covers/n01.jpg"
        );
    }

    #[test]
    fn test_cover_skips_mangled_js_placeholder_img() {
        // Reproduces the real LRB case: the <img> is a JS placeholder whose src is a
        // protocol-relative "//images/..." template that absolutizes to the bogus
        // "https://images/..."; extraction must reject it and use the og:image cover.
        let html = r#"<html><head>
            <meta property="og:image" content="https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg">
        </head><body>
            <div class="article-issue-cover-image"><img data-appsrc="//images/4/0/1/9/30999104-1-eng-GB/478.jpg"></div>
        </body></html>"#;
        let doc = Html::parse_document(html);
        assert_eq!(
            extract_cover_uri(&doc),
            "https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/30999104-1-eng-GB/478.jpg"
        );
    }

    #[test]
    fn test_cover_prefers_dedicated_element_over_meta() {
        let html = r#"<html><head>
            <meta property="og:image" content="/storage/social/fallback.jpg">
        </head><body>
            <div class="article-issue-cover-image"><img data-appsrc="/storage/covers/real.jpg"></div>
        </body></html>"#;
        let doc = Html::parse_document(html);
        assert_eq!(
            extract_cover_uri(&doc),
            "https://www.lrb.co.uk/storage/covers/real.jpg"
        );
    }

    #[test]
    fn test_cover_absent_returns_empty() {
        let doc = Html::parse_document("<html><head></head><body><p>no cover</p></body></html>");
        assert_eq!(extract_cover_uri(&doc), "");
    }

    #[test]
    fn test_extract_article_content_from_article() {
        let doc = load_html_fixture("src/test/lrb/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let adapter = LondonReviewAdapter;
        let article = adapter.extract_article(&doc, &progress);

        assert!(!article.title.is_empty(), "Article should have a title");
        assert!(
            article.body.len() > 100,
            "Article body should be long enough"
        );
    }

    #[test]
    fn test_article_title_is_cleaned() {
        let doc = load_html_fixture("src/test/lrb/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let article = LondonReviewAdapter.extract_article(&doc, &progress);
        assert_eq!(article.title, "Officer Stabler · Courting in lovely England");
    }

    #[test]
    fn test_article_byline_extracted() {
        let doc = load_html_fixture("src/test/lrb/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let article = LondonReviewAdapter.extract_article(&doc, &progress);
        assert_eq!(article.byline.as_deref(), Some("Jane Doe"));
    }

    #[test]
    fn test_discover_issues_from_archive() {
        let doc = load_html_fixture("src/test/lrb/archive.html");
        let progress = Progress::new(Verbosity::Quiet);
        let issues = LondonReviewAdapter.discover_issues(&doc, &progress);

        assert_eq!(
            issues,
            vec![
                "https://www.lrb.co.uk/the-paper/v48/n02",
                "https://www.lrb.co.uk/the-paper/v48/n01",
                "https://www.lrb.co.uk/the-paper/v47/n24",
                "https://www.lrb.co.uk/the-paper/v47/n23",
            ],
            "Should return canonical issue URLs only, deduped and absolutized"
        );
    }
}
