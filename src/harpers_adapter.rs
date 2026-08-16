use crate::adapter::{ArticleData, IssueData, MagazineAdapter};
use crate::progress::Progress;
use crate::scrape::{best_from_srcset, meta_content};
use crate::validation::{absolutize, is_valid_http_url};
use regex::Regex;
use scraper::{Html, Selector};
use std::collections::HashSet;

const BASE: &str = "https://harpers.org";

pub struct HarpersAdapter;

impl MagazineAdapter for HarpersAdapter {
    fn extract_issue(&self, doc: &Html, progress: &Progress) -> IssueData {
        let issue_article_sel =
            Selector::parse("section.issue-articles div.issue-article").unwrap();
        let reading_item_sel = Selector::parse("section.issue-readings div.reading-item").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let ac_tax_sel = Selector::parse("span.ac-tax").unwrap();
        let title_selector = Selector::parse("title").unwrap();

        let mut seen: HashSet<String> = HashSet::new();

        // Collect Readings section links in DOM order
        let reading_links: Vec<String> = doc
            .select(&reading_item_sel)
            .filter_map(|item| {
                item.select(&a_sel)
                    .filter_map(|a| a.value().attr("href"))
                    .find(|href| {
                        let parts: Vec<&str> = href.trim_matches('/').split('/').collect();
                        parts.len() >= 4 && parts.first() == Some(&"archive")
                    })
                    .map(|href| absolutize(BASE, href))
            })
            .collect();

        // Process issue-articles in DOM order (already in magazine sequence).
        // Insert Readings after the Harper's Index card.
        let mut links: Vec<String> = Vec::new();
        let mut readings_inserted = false;

        for article_el in doc.select(&issue_article_sel) {
            let is_index = article_el
                .select(&ac_tax_sel)
                .next()
                .map(|el| el.text().collect::<String>().contains("Index"))
                .unwrap_or(false);

            // Prefer /archive/ link; fall back to /harpers-index/ for the Index card.
            let href_opt = article_el
                .select(&a_sel)
                .filter_map(|a| a.value().attr("href"))
                .find(|href| {
                    let parts: Vec<&str> = href.trim_matches('/').split('/').collect();
                    parts.len() >= 4 && parts.first() == Some(&"archive")
                })
                .or_else(|| {
                    if is_index {
                        article_el
                            .select(&a_sel)
                            .filter_map(|a| a.value().attr("href"))
                            .find(|href| href.contains("/harpers-index/"))
                    } else {
                        None
                    }
                });

            if let Some(href) = href_opt {
                let url = absolutize(BASE, href);
                if seen.insert(url.clone()) {
                    links.push(url);
                }
            }

            // Insert all Readings articles immediately after Harper's Index.
            if is_index && !readings_inserted {
                readings_inserted = true;
                for r_url in &reading_links {
                    if seen.insert(r_url.clone()) {
                        links.push(r_url.clone());
                    }
                }
            }
        }

        // Fallback: if the issue had no Harper's Index card, the Readings links were
        // never inserted above — append them so they aren't silently dropped.
        if !readings_inserted {
            for r_url in &reading_links {
                if seen.insert(r_url.clone()) {
                    links.push(r_url.clone());
                }
            }
        }

        let title = doc
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>())
            .map(|t| {
                // "February 2026 | Harper's Magazine" → "February 2026"
                t.split_once(" | ")
                    .map(|(month, _)| month.trim().to_string())
                    .unwrap_or(t)
            })
            .unwrap_or_else(|| "Untitled".into());

        let cover_image_uri = extract_cover_uri(doc);
        if cover_image_uri.is_empty() {
            progress.warn("No cover image found on the issue page.");
        }

        let extra_articles = extract_artwork(doc, progress);

        progress.verbose(&format!("Found {} article links", links.len()));
        progress.verbose(&format!("Issue title: {}", title));
        progress.verbose(&format!("Cover image: {}", cover_image_uri));

        IssueData {
            links,
            title,
            cover_image_uri,
            publication_name: "Harper's Magazine".to_string(),
            extra_articles,
        }
    }

    fn extract_article(&self, doc: &Html, progress: &Progress) -> ArticleData {
        let title_selector = Selector::parse("h1.article-title").unwrap();
        let fallback_title_selector = Selector::parse("title").unwrap();
        let body_selector = Selector::parse("div.wysiwyg-content.entry-content").unwrap();

        let title = doc
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                doc.select(&fallback_title_selector).next().map(|el| {
                    let t = el.text().collect::<String>();
                    // "Some Article | Harper's Magazine" → "Some Article"
                    t.split_once(" | ")
                        .map(|(head, _)| head.trim().to_string())
                        .unwrap_or_else(|| t.trim().to_string())
                })
            })
            .unwrap_or_else(|| "Untitled".into());

        let body = doc
            .select(&body_selector)
            .map(|el| strip_chrome(el.inner_html(), &el))
            .collect::<Vec<_>>()
            .join("\n\n");

        let byline = extract_byline(doc);

        progress.verbose(&format!("Extracted: {}", title));

        ArticleData {
            title,
            byline,
            body,
        }
    }

    fn discover_issues(&self, doc: &Html, progress: &Progress) -> Vec<String> {
        let a_sel = Selector::parse("a").unwrap();
        // Canonical issue links only: /archive/YYYY/MM (no article slug after the month).
        let re = Regex::new(r"^(?:https://harpers\.org)?/archive/\d{4}/\d{2}/?$").unwrap();

        let mut seen = HashSet::new();
        let mut issues = Vec::new();
        for el in doc.select(&a_sel) {
            if let Some(href) = el.value().attr("href")
                && re.is_match(href.trim())
            {
                // Normalize with a trailing slash — Harper's canonical form.
                let url = format!("{}/", absolutize(BASE, href.trim()).trim_end_matches('/'));
                if seen.insert(url.clone()) {
                    issues.push(url);
                }
            }
        }
        progress.verbose(&format!("Discovered {} issue links on this page", issues.len()));
        issues
    }

    fn next_archive_page(&self, doc: &Html, progress: &Progress) -> Option<String> {
        // Harper's back-issues archive is WordPress-paginated. Try the standard
        // pagination markers in order of reliability.
        const NEXT_SELECTORS: &[&str] = &[
            r#"link[rel="next"]"#,
            r#"a[rel="next"]"#,
            "a.next.page-numbers",
            ".pagination a.next",
            ".nav-links a.next",
        ];
        for sel in NEXT_SELECTORS {
            let Ok(selector) = Selector::parse(sel) else {
                continue;
            };
            for el in doc.select(&selector) {
                if let Some(href) = el.value().attr("href") {
                    let abs = absolutize(BASE, href.trim());
                    // Only follow pagination within the issues archive.
                    if is_valid_http_url(&abs) && (abs.contains("/issues") || abs.contains("/page/"))
                    {
                        progress.verbose(&format!("Next archive page: {}", abs));
                        return Some(abs);
                    }
                }
            }
        }
        None
    }
}

/// Extract the issue cover image URL, resolved to an absolute URL: the dedicated cover
/// element first (largest srcset candidate preferred), then the social-image meta tags.
fn extract_cover_uri(doc: &Html) -> String {
    const COVER_SELECTORS: &[&str] = &[
        "div.issue-cover img.cover-img",
        "div.issue-cover img",
        "img.cover-img",
        ".issue-cover-image img",
    ];

    for sel in COVER_SELECTORS {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        let Some(img) = doc.select(&selector).next() else {
            continue;
        };
        if let Some(u) = best_from_srcset(&img, BASE)
            && is_valid_http_url(&u)
        {
            return u;
        }
        if let Some(raw) = img
            .value()
            .attr("src")
            .or_else(|| img.value().attr("data-src"))
        {
            let abs = absolutize(BASE, raw.trim());
            if is_valid_http_url(&abs) {
                return abs;
            }
        }
    }

    const META_SELECTORS: &[&str] = &[
        r#"meta[property="og:image"]"#,
        r#"meta[name="twitter:image"]"#,
    ];
    for sel in META_SELECTORS {
        if let Some(content) = meta_content(doc, sel) {
            let abs = absolutize(BASE, &content);
            if is_valid_http_url(&abs) {
                return abs;
            }
        }
    }

    String::new()
}

/// Capture the issue page's artwork slideshow (`div.issue-slide` cards) as a ready-made
/// gallery article: each slide becomes a `<figure>` with its image and caption. Returns
/// an empty vec when the issue has no slideshow.
fn extract_artwork(doc: &Html, progress: &Progress) -> Vec<ArticleData> {
    let slide_sel = Selector::parse("div.issue-slide").unwrap();
    let img_sel = Selector::parse(".image-holder img, img").unwrap();
    let caption_sel = Selector::parse(".caption-text").unwrap();

    let mut figures = String::new();
    let mut count = 0usize;
    for slide in doc.select(&slide_sel) {
        let Some(img) = slide.select(&img_sel).next() else {
            continue;
        };
        let src = best_from_srcset(&img, BASE).or_else(|| {
            img.value()
                .attr("src")
                .or_else(|| img.value().attr("data-src"))
                .map(|s| absolutize(BASE, s.trim()))
        });
        let Some(src) = src.filter(|s| is_valid_http_url(s)) else {
            continue;
        };

        let caption = slide
            .select(&caption_sel)
            .next()
            .map(|c| flatten_caption(&c.inner_html()))
            .unwrap_or_default();

        figures.push_str(&format!(
            "<figure class=\"issue-art\"><img src=\"{}\" alt=\"\"/>{}</figure>\n",
            src,
            if caption.is_empty() {
                String::new()
            } else {
                format!("<figcaption>{}</figcaption>", caption)
            }
        ));
        count += 1;
    }

    if count == 0 {
        return Vec::new();
    }
    progress.verbose(&format!("Captured {} artwork slides", count));
    vec![ArticleData {
        title: "Artwork from this Issue".to_string(),
        byline: None,
        body: figures,
    }]
}

/// Flatten a slideshow caption's markup: the raw HTML is nested in PDF-export div soup
/// (`div.page > div.section > …`), so drop the block wrappers while keeping inline markup
/// (`<em>`, `<a>`), then collapse whitespace and stray `&nbsp;`s.
fn flatten_caption(raw: &str) -> String {
    let no_divs = Regex::new(r"(?is)</?div[^>]*>").unwrap().replace_all(raw, " ");
    let no_nbsp = no_divs.replace("&nbsp;", " ");
    Regex::new(r"\s+")
        .unwrap()
        .replace_all(no_nbsp.trim(), " ")
        .to_string()
}

/// Extract the author byline. Harper's links authors as `/author/<slug>/`; the byline is
/// the author link(s) in the article header. Searched header containers first so related-
/// article cards further down the page can't hijack the byline; falls back to the first
/// author link in the document (headers precede everything else in source order).
fn extract_byline(doc: &Html) -> Option<String> {
    const HEADER_SCOPES: &[&str] = &[
        r#".article-header a[href*="/author/"]"#,
        r#".title-header a[href*="/author/"]"#,
        r#"header a[href*="/author/"]"#,
        r#".byline a[href*="/author/"]"#,
        r#".header-meta a[href*="/author/"]"#,
    ];
    for sel in HEADER_SCOPES {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        let mut names: Vec<String> = Vec::new();
        for a in doc.select(&selector) {
            let name = a.text().collect::<String>().trim().to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        if !names.is_empty() {
            return Some(names.join(", "));
        }
    }

    let fallback = Selector::parse(r#"a[href*="/author/"]"#).ok()?;
    doc.select(&fallback)
        .next()
        .map(|a| a.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Remove non-content chrome (Adjust/Share controls, sharing widgets, related-article
/// promos, newsletter signups) from an article body by deleting those subtrees' HTML.
fn strip_chrome(mut raw: String, scope: &scraper::ElementRef<'_>) -> String {
    const CHROME_SELECTORS: &[&str] = &[
        "div.header-meta",
        ".article-tools",
        ".sharing",
        ".share-buttons",
        ".related-articles",
        ".newsletter-signup",
        ".after-post",
    ];
    for sel in CHROME_SELECTORS {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        for el in scope.select(&selector) {
            raw = raw.replacen(&el.html(), "", 1);
        }
    }
    raw
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
    fn test_extract_article_links_from_harpers_issue() {
        let doc = load_html_fixture("src/test/harpers/issue.html");
        let progress = Progress::new(Verbosity::Quiet);
        let adapter = HarpersAdapter;
        let issue = adapter.extract_issue(&doc, &progress);

        assert!(
            !issue.links.is_empty(),
            "Expected at least one article link"
        );
        assert_eq!(issue.title, "February 2026");
        assert!(
            issue.links.iter().all(|l| {
                l.starts_with("https://harpers.org/archive/")
                    || l.starts_with("https://harpers.org/harpers-index/")
            }),
            "All links should be absolute Harper's article URLs"
        );
        assert!(!issue.cover_image_uri.is_empty());
    }

    #[test]
    fn test_issue_cover_from_cover_img() {
        let doc = load_html_fixture("src/test/harpers/issue.html");
        let progress = Progress::new(Verbosity::Quiet);
        let issue = HarpersAdapter.extract_issue(&doc, &progress);
        assert_eq!(
            issue.cover_image_uri,
            "https://wp.harpers.org/wp-content/uploads/2026/03/001_HA0526-0C1DS.jpg"
        );
    }

    #[test]
    fn test_issue_artwork_slideshow_captured() {
        let doc = load_html_fixture("src/test/harpers/issue.html");
        let progress = Progress::new(Verbosity::Quiet);
        let issue = HarpersAdapter.extract_issue(&doc, &progress);

        assert_eq!(issue.extra_articles.len(), 1, "Expected one artwork gallery");
        let art = &issue.extra_articles[0];
        assert_eq!(art.title, "Artwork from this Issue");
        assert!(
            art.body
                .contains("https://wp.harpers.org/wp-content/uploads/2026/03/CUT-7-2400.jpg"),
            "Gallery should contain the slide image"
        );
        assert!(
            art.body.contains("<em>Predatory Drift</em>"),
            "Caption should keep inline markup: {}",
            art.body
        );
        assert!(
            !art.body.contains("layoutArea"),
            "Caption div soup should be flattened"
        );
    }

    #[test]
    fn test_extract_article_content_from_harpers_article() {
        let doc = load_html_fixture("src/test/harpers/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let adapter = HarpersAdapter;
        let article = adapter.extract_article(&doc, &progress);

        assert!(!article.title.is_empty(), "Article should have a title");
        assert!(
            article.body.len() > 100,
            "Article body should be long enough"
        );
    }

    #[test]
    fn test_article_body_excludes_adjust_share_controls() {
        let doc = load_html_fixture("src/test/harpers/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let adapter = HarpersAdapter;
        let article = adapter.extract_article(&doc, &progress);

        assert!(
            !article.body.contains("header-meta"),
            "Article body should not contain the Adjust/Share UI controls"
        );
    }

    #[test]
    fn test_article_byline_from_author_link() {
        let doc = load_html_fixture("src/test/harpers/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let article = HarpersAdapter.extract_article(&doc, &progress);
        assert_eq!(article.byline.as_deref(), Some("Samuel Moyn"));
    }

    #[test]
    fn test_article_body_keeps_images() {
        let doc = load_html_fixture("src/test/harpers/article.html");
        let progress = Progress::new(Verbosity::Quiet);
        let article = HarpersAdapter.extract_article(&doc, &progress);
        assert!(
            article.body.contains("CUT-9-1-768x888.jpg"),
            "Mid-article art should stay in the body for embedding"
        );
    }

    #[test]
    fn test_discover_issues_from_archive() {
        let doc = load_html_fixture("src/test/harpers/issues_archive.html");
        let progress = Progress::new(Verbosity::Quiet);
        let issues = HarpersAdapter.discover_issues(&doc, &progress);
        assert_eq!(
            issues,
            vec![
                "https://harpers.org/archive/2026/05/",
                "https://harpers.org/archive/2026/04/",
                "https://harpers.org/archive/2026/03/",
            ],
            "Issue links only — article links must be excluded"
        );
    }

    #[test]
    fn test_next_archive_page_wordpress_pagination() {
        let doc = load_html_fixture("src/test/harpers/issues_archive.html");
        let progress = Progress::new(Verbosity::Quiet);
        let next = HarpersAdapter.next_archive_page(&doc, &progress);
        assert_eq!(next.as_deref(), Some("https://harpers.org/issues/page/2/"));
    }

    #[test]
    fn test_next_archive_page_none_on_last_page() {
        let html = r#"<html><body>
            <a href="/archive/2000/01/">Jan 2000</a>
            <div class="pagination"><span class="current">99</span></div>
        </body></html>"#;
        let doc = Html::parse_document(html);
        let progress = Progress::new(Verbosity::Quiet);
        assert_eq!(HarpersAdapter.next_archive_page(&doc, &progress), None);
    }
}
