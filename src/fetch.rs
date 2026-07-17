use crate::adapter::ArticleData;
use crate::progress::{Progress, ProgressBar};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use scraper::Html;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{thread, time::Duration};

/// A browser-like User-Agent. Some publications reject the default reqwest agent.
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Number of attempts for a single network request before giving up.
const MAX_ATTEMPTS: u32 = 4;

pub fn make_client(cookie: Option<&str>) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(UA));
    if let Some(cookie_str) = cookie
        && !cookie_str.is_empty()
    {
        headers.insert(COOKIE, HeaderValue::from_str(cookie_str)?);
    }
    Ok(Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// Fetch a URL and parse it as an HTML document.
pub fn fetch_html_body(
    client: &Client,
    url: &str,
    delay: &u64,
    progress: &Progress,
) -> Result<Html> {
    let body = fetch_html_raw(client, url, delay, progress)?;
    Ok(Html::parse_document(&body))
}

/// Fetch a URL as text, sleeping `delay` ms first (politeness) and retrying transient
/// failures with exponential backoff.
pub fn fetch_html_raw(
    client: &Client,
    url: &str,
    delay: &u64,
    progress: &Progress,
) -> Result<String> {
    progress.verbose(&format!("GET {}", url));
    thread::sleep(Duration::from_millis(*delay));

    let body = with_retry(progress, url, || {
        let resp = client.get(url).send()?.error_for_status()?;
        Ok(resp.text()?)
    })?;

    progress.verbose(&format!("{} bytes received", body.len()));
    Ok(body)
}

/// Download a URL as raw bytes, returning the bytes and a best-effort image MIME type.
/// Retries transient failures with backoff. `referer`, when set, is sent as the
/// `Referer` header — many sites hotlink-protect images and 403 requests without one.
pub fn download_bytes(
    client: &Client,
    url: &str,
    referer: Option<&str>,
    progress: &Progress,
) -> Result<(Vec<u8>, String)> {
    progress.verbose(&format!("Downloading asset: {}", url));

    with_retry(progress, url, || {
        let mut req = client.get(url);
        if let Some(r) = referer {
            req = req.header(REFERER, r);
        }
        let resp = req.send()?.error_for_status()?;

        let header_mime = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
            .filter(|s| s.starts_with("image/"));

        let bytes = resp.bytes()?.to_vec();
        let mime = header_mime.unwrap_or_else(|| guess_image_mime(url, &bytes));
        Ok((bytes, mime))
    })
}

/// Fetch and parse every article in `links` using up to `concurrency` worker threads,
/// preserving the original order. Each request still sleeps `delay` ms first for
/// politeness. A failed article is logged and skipped rather than aborting the run.
///
/// `extract` maps a parsed article document to `ArticleData`; it must be `Sync` because
/// it is shared across the worker threads.
pub fn fetch_articles<F>(
    client: &Client,
    links: &[String],
    delay: u64,
    concurrency: usize,
    progress: &Progress,
    extract: F,
) -> Vec<ArticleData>
where
    F: Fn(&Html) -> ArticleData + Sync,
{
    let total = links.len();
    let workers = concurrency.clamp(1, total.max(1));
    let next = AtomicUsize::new(0);
    let bar = ProgressBar::new(progress, total);
    let results: Mutex<Vec<Option<ArticleData>>> =
        Mutex::new((0..total).map(|_| None).collect());

    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= total {
                        break;
                    }
                    let url = &links[i];
                    match fetch_html_body(client, url, &delay, progress) {
                        Ok(doc) => {
                            let article = extract(&doc);
                            bar.inc(&article.title);
                            results.lock().unwrap()[i] = Some(article);
                        }
                        Err(e) => {
                            bar.inc("(failed)");
                            progress.warn(&format!("Skipping article {}: {}", url, e));
                        }
                    }
                }
            });
        }
    });
    bar.finish();

    results.into_inner().unwrap().into_iter().flatten().collect()
}

/// Guess an image MIME type from the file extension, falling back to magic bytes,
/// then to JPEG.
fn guess_image_mime(url: &str, bytes: &[u8]) -> String {
    let lower = url.split(['?', '#']).next().unwrap_or(url).to_lowercase();
    if lower.ends_with(".png") {
        return "image/png".into();
    } else if lower.ends_with(".gif") {
        return "image/gif".into();
    } else if lower.ends_with(".webp") {
        return "image/webp".into();
    } else if lower.ends_with(".svg") {
        return "image/svg+xml".into();
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        return "image/jpeg".into();
    }

    // Fall back to magic-byte sniffing.
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png".into(),
        [0x47, 0x49, 0x46, ..] => "image/gif".into(),
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg".into(),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp".into(),
        _ => "image/jpeg".into(),
    }
}

/// Run `op`, retrying on error with exponential backoff (2s, 4s, 8s…).
fn with_retry<T, F>(progress: &Progress, url: &str, mut op: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    let mut attempt = 1;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < MAX_ATTEMPTS => {
                let backoff = 2u64.pow(attempt);
                progress.verbose(&format!(
                    "Request to {} failed (attempt {}/{}): {}. Retrying in {}s…",
                    url, attempt, MAX_ATTEMPTS, e, backoff
                ));
                thread::sleep(Duration::from_secs(backoff));
                attempt += 1;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("giving up on {} after {} attempts", url, MAX_ATTEMPTS)
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guess_mime_from_extension() {
        assert_eq!(guess_image_mime("https://x/y/cover.png", &[]), "image/png");
        assert_eq!(guess_image_mime("https://x/y/cover.JPG", &[]), "image/jpeg");
        assert_eq!(
            guess_image_mime("https://x/y/cover.jpeg?v=2", &[]),
            "image/jpeg"
        );
        assert_eq!(
            guess_image_mime("https://x/y/cover.webp#frag", &[]),
            "image/webp"
        );
    }

    #[test]
    fn test_guess_mime_from_magic_bytes() {
        assert_eq!(guess_image_mime("https://x/y/noext", &[0x89, b'P', b'N', b'G']), "image/png");
        assert_eq!(
            guess_image_mime("https://x/y/noext", &[0xFF, 0xD8, 0xFF, 0x00]),
            "image/jpeg"
        );
    }

    #[test]
    fn test_guess_mime_default() {
        assert_eq!(guess_image_mime("https://x/y/mystery", &[0x00, 0x01]), "image/jpeg");
    }
}
