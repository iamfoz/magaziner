use crate::adapter::{ArticleData, IssueData, MagazineAdapter};
use crate::progress::Progress;
use crate::validation::absolutize;
use regex::Regex;
use scraper::{Html, Selector};

const BASE: &str = "https://www.lrb.co.uk";

pub struct LondonReviewAdapter;

impl MagazineAdapter for LondonReviewAdapter {
    fn extract_issue(&self, doc: &Html, progress: &Progress) -> IssueData {
        let articles_selector = Selector::parse("a.toc-item").unwrap();
        let title_selector = Selector::parse("title").unwrap();
        let cover_selector = Selector::parse("div.article-issue-cover-image img").unwrap();

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

        // Prefer the full-resolution lazy-load source; fall back to srcset then src.
        // Resolve to an absolute URL so relative `/storage/...` paths still download.
        let cover_image_uri = doc
            .select(&cover_selector)
            .next()
            .and_then(|img| {
                img.value()
                    .attr("data-appsrc")
                    .or_else(|| img.value().attr("srcset"))
                    .or_else(|| img.value().attr("src"))
            })
            .map(|url| url.split_whitespace().next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
            .map(|s| absolutize(BASE, &s))
            .unwrap_or_default();

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
    fn test_cover_uri_is_absolutized() {
        let doc = load_html_fixture("src/test/lrb/issue.html");
        let progress = Progress::new(Verbosity::Quiet);
        let issue = LondonReviewAdapter.extract_issue(&doc, &progress);
        // data-appsrc is a relative /storage/... path; it must become absolute.
        assert_eq!(
            issue.cover_image_uri,
            "https://www.lrb.co.uk/storage/2000_filter/images/4/0/1/9/cover.jpg"
        );
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
