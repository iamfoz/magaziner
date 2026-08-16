use crate::adapter::ArticleData;
use crate::fetch::download_bytes;
use crate::progress::Progress;
use crate::style::STYLESHEET;
use crate::validation::absolutize;
use anyhow::{Context, Result};
use ego_tree::NodeRef;
use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};
use reqwest::blocking::Client;
use scraper::Html;
use scraper::node::Node;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;

/// Elements whose entire subtree is dropped from article bodies (scripts, embeds,
/// site chrome) — they have no place in a clean reading EPUB.
const SKIP_ELEMENTS: &[&str] = &[
    "script", "style", "iframe", "noscript", "form", "object", "embed", "video", "audio",
    "canvas", "button", "input", "select", "textarea", "svg", "link", "meta", "base", "nav",
    "head", "title",
];

/// Structural wrappers that `Html::parse_fragment` inserts (or that a source page may
/// contain): emit their children but not the tag itself, so bodies don't end up with a
/// stray `<html>`/`<body>` nested inside the article.
const TRANSPARENT_ELEMENTS: &[&str] = &["html", "body"];

/// HTML void elements — emitted self-closing for XHTML validity.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
    "source", "track", "wbr",
];

#[allow(clippy::too_many_arguments)]
pub fn build_epub(
    progress: &Progress,
    title: &str,
    publication_name: &str,
    output_path: &Path,
    articles: Vec<ArticleData>,
    cover_image_uri: &str,
    client: &Client,
    base_url: &str,
) -> Result<()> {
    let mut epub = EpubBuilder::new(ZipLibrary::new()?)?;
    epub.metadata("title", title)?
        .metadata("author", publication_name)?
        .metadata("lang", "en")?
        .metadata("generator", "magaziner")?
        .metadata(
            "description",
            format!("{} — {}", publication_name, title),
        )?;

    // Curated e-reader stylesheet, linked from every content document.
    epub.stylesheet(STYLESHEET.as_bytes())?;

    // Cover: download straight to memory (no temp file), detect the real MIME type.
    progress.step("Downloading cover…");
    if !cover_image_uri.trim().is_empty() && cover_image_uri.starts_with("http") {
        match download_bytes(client, cover_image_uri, Some(base_url), progress) {
            Ok((bytes, mime)) => {
                let cover_name = format!("cover.{}", ext_for_mime(&mime));
                epub.add_cover_image(&cover_name, bytes.as_slice(), mime.as_str())?;
            }
            Err(e) => progress.warn(&format!("Could not download cover: {}", e)),
        }
    } else if !cover_image_uri.trim().is_empty() {
        progress.warn(&format!("Skipping non-absolute cover URL: {}", cover_image_uri));
    }

    progress.step("Building EPUB…");

    // Title page.
    let title_page = wrap_document(
        title,
        &format!(
            r#"<body class="title-page">
    <h1>{}</h1>
    <h3>{}</h3>
  </body>"#,
            escape_text(title),
            escape_text(publication_name)
        ),
    );
    epub.add_content(
        EpubContent::new("title.xhtml", title_page.as_bytes())
            .title("Title Page")
            .reftype(ReferenceType::TitlePage),
    )?;

    // Table of contents.
    let mut list_items = String::new();
    for (i, article) in articles.iter().enumerate() {
        list_items.push_str(&format!(
            "      <li><a href=\"article{}.xhtml\">{}</a></li>\n",
            i,
            escape_text(&article.title)
        ));
    }
    let toc_body = format!(
        r#"<body>
    <h2>Contents</h2>
    <ol class="toc">
{}    </ol>
  </body>"#,
        list_items
    );
    epub.add_content(
        EpubContent::new("toc.xhtml", wrap_document("Contents", &toc_body).as_bytes())
            .title("Contents")
            .reftype(ReferenceType::Toc),
    )?;

    // Articles.
    let mut image_counter = 0usize;
    for (i, article) in articles.into_iter().enumerate() {
        progress.verbose(&format!("Adding article: {}", article.title));

        let image_map =
            download_article_images(&article.body, base_url, client, progress, i, &mut image_counter, &mut epub)?;
        let body_xhtml = sanitize_body_to_xhtml(&article.body, &image_map, base_url);

        let byline_html = article
            .byline
            .as_deref()
            .filter(|b| !b.is_empty())
            .map(|b| format!("    <p class=\"byline\">By {}</p>\n", escape_text(b)))
            .unwrap_or_default();

        // The body is wrapped in .article-body so styling (e.g. the drop cap) can target
        // the first real article paragraph without ever touching the byline or title.
        let content = format!(
            r#"<body>
    <h1 class="article-title">{}</h1>
{}    <div class="article-body">
    {}
    </div>
  </body>"#,
            escape_text(&article.title),
            byline_html,
            body_xhtml
        );

        let filename = format!("article{}.xhtml", i);
        epub.add_content(
            EpubContent::new(filename, wrap_document(&article.title, &content).as_bytes())
                .title(&article.title)
                .reftype(ReferenceType::Text),
        )?;
    }

    // Write atomically: generate into a temp file next to the target, then rename.
    progress.step("Saving EPUB…");
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = output_path.with_extension("epub.tmp");
    let generate = || -> Result<()> {
        let file = File::create(&tmp_path)
            .with_context(|| format!("creating {}", tmp_path.display()))?;
        epub.generate(file)?;
        fs::rename(&tmp_path, output_path)?;
        Ok(())
    };
    // Any failure after the temp file is created (generation or rename) removes it, so no
    // orphaned `.epub.tmp` debris is left in the output directory.
    generate().inspect_err(|_| {
        let _ = fs::remove_file(&tmp_path);
    })?;

    progress.done(output_path.display());
    Ok(())
}

/// Wrap body markup in a well-formed XHTML document that links the shared stylesheet.
fn wrap_document(title: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en">
  <head>
    <title>{}</title>
    <link rel="stylesheet" type="text/css" href="stylesheet.css"/>
  </head>
  {}
</html>"#,
        escape_text(title),
        body
    )
}

/// Find every `<img>` in `body`, download each unique image, register it as an EPUB
/// resource, and return a map from the image's original `src` to its in-EPUB href.
/// Images that fail to download map to `None` (and are dropped during serialization).
#[allow(clippy::too_many_arguments)]
fn download_article_images(
    body: &str,
    base_url: &str,
    client: &Client,
    progress: &Progress,
    article_idx: usize,
    counter: &mut usize,
    epub: &mut EpubBuilder<ZipLibrary>,
) -> Result<HashMap<String, Option<String>>> {
    use scraper::Selector;
    let fragment = Html::parse_fragment(body);
    let img_sel = Selector::parse("img").unwrap();

    let mut map: HashMap<String, Option<String>> = HashMap::new();
    for img in fragment.select(&img_sel) {
        let Some(src) = img.value().attr("src") else {
            continue;
        };
        if src.trim().is_empty() || map.contains_key(src) {
            continue;
        }
        let abs = absolutize(base_url, src);
        match download_bytes(client, &abs, Some(base_url), progress) {
            Ok((bytes, mime)) => {
                let href = format!("images/a{}_{}.{}", article_idx, counter, ext_for_mime(&mime));
                *counter += 1;
                epub.add_resource(&href, bytes.as_slice(), mime.as_str())?;
                map.insert(src.to_string(), Some(href));
            }
            Err(e) => {
                progress.warn(&format!("Dropping image {} ({})", abs, e));
                map.insert(src.to_string(), None);
            }
        }
    }
    Ok(map)
}

/// Maximum DOM nesting depth we serialize. Real articles are shallow; this is a guard
/// against pathologically deep (or maliciously crafted) markup blowing the stack.
const MAX_DEPTH: usize = 256;

/// Parse an article body fragment and re-serialize it as clean, valid XHTML: text is
/// XML-escaped, void elements are self-closed, disallowed elements are stripped, `<a>`
/// links are absolutized against `base_url`, and `<img>` tags are rewritten to their
/// embedded resource href (or dropped if missing).
fn sanitize_body_to_xhtml(
    body: &str,
    images: &HashMap<String, Option<String>>,
    base_url: &str,
) -> String {
    let fragment = Html::parse_fragment(body);
    let mut out = String::new();
    for child in fragment.tree.root().children() {
        serialize_node(child, &mut out, images, base_url, 0);
    }
    out
}

fn serialize_node(
    node: NodeRef<Node>,
    out: &mut String,
    images: &HashMap<String, Option<String>>,
    base_url: &str,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match node.value() {
        Node::Text(t) => out.push_str(&escape_text(t)),
        Node::Element(el) => {
            let name = el.name();

            if TRANSPARENT_ELEMENTS.contains(&name) {
                for child in node.children() {
                    serialize_node(child, out, images, base_url, depth + 1);
                }
                return;
            }

            if SKIP_ELEMENTS.contains(&name) {
                return;
            }

            if name == "img" {
                if let Some(src) = el.attr("src")
                    && let Some(Some(href)) = images.get(src)
                {
                    let alt = el.attr("alt").unwrap_or("");
                    out.push_str(&format!(
                        "<img src=\"{}\" alt=\"{}\"/>",
                        escape_attr(href),
                        escape_attr(alt)
                    ));
                }
                return;
            }

            out.push('<');
            out.push_str(name);
            for (k, v) in el.attrs() {
                if keep_attr(name, k) {
                    // Resolve relative <a href> against the source so links aren't left
                    // dangling relative to the EPUB's internal article files.
                    let value = if name == "a" && k == "href" {
                        absolutize(base_url, v)
                    } else {
                        v.to_string()
                    };
                    out.push_str(&format!(" {}=\"{}\"", k, escape_attr(&value)));
                }
            }

            if VOID_ELEMENTS.contains(&name) {
                out.push_str("/>");
                return;
            }
            out.push('>');
            for child in node.children() {
                serialize_node(child, out, images, base_url, depth + 1);
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
        // Root / fragment wrapper nodes: descend into their children.
        Node::Document | Node::Fragment => {
            for child in node.children() {
                serialize_node(child, out, images, base_url, depth + 1);
            }
        }
        // Comments, doctypes, processing instructions: drop.
        _ => {}
    }
}

/// A conservative attribute whitelist. Anything not listed is dropped to keep the XHTML
/// clean and valid (no inline styles, event handlers, or stray site attributes).
fn keep_attr(element: &str, attr: &str) -> bool {
    match attr {
        "class" | "id" | "colspan" | "rowspan" | "lang" => true,
        "href" if element == "a" => true,
        "cite" if element == "blockquote" || element == "q" => true,
        _ => false,
    }
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            // Strip other C0 control characters — illegal in XML 1.0 and rejected by
            // strict EPUB validators.
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "jpg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BASE: &str = "https://www.lrb.co.uk/the-paper/v1/n1";

    fn no_images() -> HashMap<String, Option<String>> {
        HashMap::new()
    }

    /// End-to-end EPUB generation, gated behind `MAGAZINER_TEST_OUT=<path>` so it only
    /// runs when explicitly asked (it writes a real file). Uses image-free bodies and an
    /// empty cover URL so it performs zero network I/O.
    #[test]
    fn test_build_epub_end_to_end() {
        let Ok(out) = std::env::var("MAGAZINER_TEST_OUT") else {
            return;
        };
        let progress = crate::progress::Progress::new(crate::progress::Verbosity::Quiet);
        let client = crate::fetch::make_client(None).unwrap();
        let articles = vec![
            ArticleData {
                title: "Pride & Prejudice < Sense".to_string(),
                byline: Some("Jane Doe".to_string()),
                body: "<p>First &amp; foremost.</p><blockquote>A quote<br>line two</blockquote><hr>"
                    .to_string(),
            },
            ArticleData {
                title: "Second Article".to_string(),
                byline: None,
                body: "<p>Body two <em>emphasis</em> and <a href=\"https://x.com\">a link</a>.</p>"
                    .to_string(),
            },
        ];
        let path = std::path::Path::new(&out);
        build_epub(
            &progress,
            "Vol. 99 No. 3 · 15 March 2025 & More",
            "London Review of Books",
            path,
            articles,
            "",
            &client,
            "https://www.lrb.co.uk/the-paper/v99/n03",
        )
        .unwrap();
        assert!(path.exists(), "epub was not written");
    }

    #[test]
    fn test_artwork_gallery_body_sanitizes_cleanly() {
        // The Harper's artwork gallery body: figure + embedded image + figcaption with
        // inline markup. With the image successfully embedded, everything survives.
        let body = r#"<figure class="issue-art"><img src="https://wp.harpers.org/a.jpg" alt=""/><figcaption><em>Predatory Drift</em>, a painting by Rachel Simon Marino</figcaption></figure>"#;
        let mut images = HashMap::new();
        images.insert(
            "https://wp.harpers.org/a.jpg".to_string(),
            Some("images/a0_0.jpeg".to_string()),
        );
        let out = sanitize_body_to_xhtml(body, &images, TEST_BASE);
        assert!(out.contains(r#"<figure class="issue-art">"#), "got: {}", out);
        assert!(out.contains(r#"<img src="images/a0_0.jpeg""#), "got: {}", out);
        assert!(out.contains("<em>Predatory Drift</em>"), "got: {}", out);

        // If the image download failed, the figure degrades to caption-only but stays valid.
        let out = sanitize_body_to_xhtml(body, &no_images(), TEST_BASE);
        assert!(!out.contains("<img"), "got: {}", out);
        assert!(out.contains("<figcaption>"), "got: {}", out);
    }

    #[test]
    fn test_escape_text() {
        assert_eq!(escape_text("A & B < C > D"), "A &amp; B &lt; C &gt; D");
    }

    #[test]
    fn test_void_elements_self_close() {
        let out = sanitize_body_to_xhtml("<p>one<br>two<hr></p>", &no_images(), TEST_BASE);
        assert!(out.contains("<br/>"), "got: {}", out);
        assert!(out.contains("<hr/>"), "got: {}", out);
        assert!(!out.contains("<br>"), "should not contain non-closed br: {}", out);
    }

    #[test]
    fn test_no_html_body_wrapper_leaks() {
        // Html::parse_fragment wraps content in <html>; it must not appear in the output.
        let out = sanitize_body_to_xhtml("<p>hello <strong>world</strong></p><h2>Head</h2>", &no_images(), TEST_BASE);
        assert!(!out.contains("<html"), "leaked <html>: {}", out);
        assert!(!out.contains("<body"), "leaked <body>: {}", out);
        assert!(out.starts_with("<p>hello "), "got: {}", out);
        assert!(out.contains("<h2>Head</h2>"), "got: {}", out);
    }

    #[test]
    fn test_scripts_and_iframes_stripped() {
        let out = sanitize_body_to_xhtml(
            "<p>keep</p><script>evil()</script><iframe src=\"x\"></iframe>",
            &no_images(),
            TEST_BASE,
        );
        assert!(out.contains("keep"));
        assert!(!out.to_lowercase().contains("script"));
        assert!(!out.to_lowercase().contains("iframe"));
    }

    #[test]
    fn test_ampersand_in_text_is_escaped() {
        let out = sanitize_body_to_xhtml("<p>Pride &amp; Prejudice</p>", &no_images(), TEST_BASE);
        assert!(out.contains("Pride &amp; Prejudice"), "got: {}", out);
        // Ensure no bare, unescaped ampersand slipped through.
        assert!(!out.contains("Pride & Prejudice"));
    }

    #[test]
    fn test_inline_style_and_event_attrs_dropped() {
        let out = sanitize_body_to_xhtml(
            "<p class=\"keep\" style=\"color:red\" onclick=\"x()\">hi</p>",
            &no_images(),
            TEST_BASE,
        );
        assert!(out.contains("class=\"keep\""), "got: {}", out);
        assert!(!out.contains("style="), "got: {}", out);
        assert!(!out.contains("onclick"), "got: {}", out);
    }

    #[test]
    fn test_missing_image_is_dropped() {
        // src not present in the (empty) image map → the img is dropped entirely.
        let out = sanitize_body_to_xhtml("<p>x</p><img src=\"/a.jpg\">", &no_images(), TEST_BASE);
        assert!(!out.contains("<img"), "got: {}", out);
        assert!(out.contains("x"));
    }

    #[test]
    fn test_embedded_image_is_rewritten() {
        let mut map = HashMap::new();
        map.insert("/a.jpg".to_string(), Some("images/a0_0.jpg".to_string()));
        let out = sanitize_body_to_xhtml("<img src=\"/a.jpg\" alt=\"pic\">", &map, TEST_BASE);
        assert!(out.contains("<img src=\"images/a0_0.jpg\" alt=\"pic\"/>"), "got: {}", out);
    }

    #[test]
    fn test_link_href_preserved() {
        let out = sanitize_body_to_xhtml("<p><a href=\"https://x.com/page\">link</a></p>", &no_images(), TEST_BASE);
        assert!(out.contains("href=\"https://x.com/page\""), "got: {}", out);
    }

    #[test]
    fn test_relative_link_href_absolutized() {
        // A site-relative footnote/cross-reference link must resolve to the source site,
        // not dangle relative to the internal articleN.xhtml file.
        let out = sanitize_body_to_xhtml("<p><a href=\"/glossary/foo\">x</a></p>", &no_images(), TEST_BASE);
        assert!(
            out.contains("href=\"https://www.lrb.co.uk/glossary/foo\""),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_control_chars_stripped() {
        // Vertical tab (U+000B) is illegal in XML 1.0 and must be dropped; tab/newline kept.
        let out = sanitize_body_to_xhtml("<p>a\u{000B}b\tc</p>", &no_images(), TEST_BASE);
        assert!(!out.contains('\u{000B}'), "control char leaked: {:?}", out);
        assert!(out.contains("a"), "got: {:?}", out);
        assert!(out.contains('\t'), "tab should be preserved: {:?}", out);
    }

    #[test]
    fn test_deep_nesting_does_not_panic() {
        // Far exceed MAX_DEPTH; must not stack-overflow, just truncate deep content.
        let body = format!("{}{}", "<div>".repeat(1000), "</div>".repeat(1000));
        let _ = sanitize_body_to_xhtml(&body, &no_images(), TEST_BASE);
    }

    #[test]
    fn test_ext_for_mime() {
        assert_eq!(ext_for_mime("image/png"), "png");
        assert_eq!(ext_for_mime("image/jpeg"), "jpg");
        assert_eq!(ext_for_mime("something/else"), "jpg");
    }
}
