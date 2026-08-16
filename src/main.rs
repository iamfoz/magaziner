mod adapter;
mod epub;
mod fetch;
mod harpers_adapter;
mod history;
mod london_review_adapter;
mod progress;
mod scrape;
mod style;
mod validation;

use adapter::{ArticleData, MagazineAdapter};
use anyhow::{Result, bail};
use clap::Parser;
use epub::build_epub;
use fetch::{fetch_articles, fetch_html_body, make_client};
use harpers_adapter::HarpersAdapter;
use history::{History, HistoryEntry};
use london_review_adapter::LondonReviewAdapter;
use progress::{Progress, Verbosity};
use reqwest::blocking::Client;
use std::path::{Path, PathBuf};
use validation::{
    MagazineSource, detect_source, issue_id_from_url, source_code, validate_magazine_url,
};

/// Default archive listing page used by `--all` to discover LRB issues. This shows the
/// current volume; discovery walks back to older volumes via the "Previous Volume" button.
const LRB_ARCHIVE_URL: &str = "https://www.lrb.co.uk/archive";

/// Harper's back-issues archive; `--all --source harpers` walks its pagination.
const HARPERS_ARCHIVE_URL: &str = "https://harpers.org/issues/";

/// Safety cap on how many archive pages `--all` will visit per source. LRB has ~48
/// volume pages; Harper's is monthly since 1850, so its paginated archive runs to a few
/// hundred pages depending on issues-per-page.
fn max_archive_pages(source: &MagazineSource) -> usize {
    match source {
        MagazineSource::LondonReview => 80,
        MagazineSource::Harpers => 400,
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "magaziner",
    version,
    about = "Generate EPUB files from magazine archives (London Review of Books, Harper's)",
    long_about = None
)]
struct Args {
    #[arg(
        short,
        long,
        value_parser = validate_magazine_url,
        required_unless_present = "all",
        help = "Magazine archive URL (LRB or Harper's)"
    )]
    url: Option<String>,

    #[arg(
        long,
        help = "Download every issue not already in the history file (see --source)",
        default_value_t = false
    )]
    all: bool,

    #[arg(
        long,
        value_parser = parse_source,
        default_value = "lrb",
        help = "Which magazine --all discovers: lrb or harpers"
    )]
    source: MagazineSource,

    #[arg(
        long,
        help = "With --all: list the issues that would be downloaded, then exit",
        default_value_t = false
    )]
    list: bool,

    #[arg(
        long,
        short,
        help = "Output directory for generated EPUBs (Ex: ./downloads)",
        default_value = "."
    )]
    output: PathBuf,

    #[arg(
        long,
        help = "Path to the download-history file [default: <output>/.magaziner-history.json]"
    )]
    history: Option<PathBuf>,

    #[arg(
        short,
        long,
        help = "Delay before each request, in milliseconds",
        default_value_t = 3000
    )]
    delay: u64,

    #[arg(
        short,
        long,
        help = "Number of concurrent article downloads",
        default_value_t = 4
    )]
    concurrency: usize,

    #[arg(
        short,
        long,
        help = "Re-download and overwrite even if the issue is already in the history",
        default_value_t = false
    )]
    force: bool,

    #[arg(short, long, help = "Print detailed logs", conflicts_with = "quiet")]
    verbose: bool,

    #[arg(short, long, help = "Suppress all output", conflicts_with = "verbose")]
    quiet: bool,

    #[arg(
        short,
        long,
        help = "Custom output filename without extension (single-issue mode only)"
    )]
    name: Option<String>,
}

/// The result of attempting one issue.
enum Outcome {
    Downloaded,
    Skipped,
}

/// Parse the `--source` flag value.
fn parse_source(s: &str) -> Result<MagazineSource, String> {
    match s.to_ascii_lowercase().as_str() {
        "lrb" | "londonreview" | "london-review" => Ok(MagazineSource::LondonReview),
        "harpers" | "harper's" | "harper" => Ok(MagazineSource::Harpers),
        other => Err(format!("unknown source '{}': use 'lrb' or 'harpers'", other)),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let verbosity = if args.verbose {
        Verbosity::Verbose
    } else if args.quiet {
        Verbosity::Quiet
    } else {
        Verbosity::Normal
    };
    let progress = Progress::new(verbosity);

    if !args.output.exists() {
        std::fs::create_dir_all(&args.output)?;
    }

    let history_path = args
        .history
        .clone()
        .unwrap_or_else(|| args.output.join(".magaziner-history.json"));
    let mut history = History::load(&history_path)?;

    if args.all {
        run_all(&args, &progress, &mut history, &history_path)
    } else {
        run_single(&args, &progress, &mut history, &history_path)
    }
}

/// Single-issue mode: download the one issue named by `--url`.
fn run_single(
    args: &Args,
    progress: &Progress,
    history: &mut History,
    history_path: &Path,
) -> Result<()> {
    let url = args.url.as_ref().expect("clap enforces url unless --all");
    let source = detect_source(url).expect("URL already validated by clap");

    let cookie = harpers_cookie_for(&source);
    let client = make_client(cookie.as_deref())?;

    match process_issue(url, args.name.clone(), &source, &client, args, progress, history, history_path)? {
        Outcome::Downloaded => {}
        Outcome::Skipped => {
            progress.info("Already downloaded. Use --force to re-download.");
        }
    }
    Ok(())
}

/// Automated mode: discover the selected source's issues and download those not yet in
/// the history.
fn run_all(
    args: &Args,
    progress: &Progress,
    history: &mut History,
    history_path: &Path,
) -> Result<()> {
    if args.name.is_some() {
        progress.warn("--name is ignored in --all mode; filenames are derived per issue.");
    }

    let source = args.source.clone();
    let cookie = harpers_cookie_for(&source);
    let client = make_client(cookie.as_deref())?;
    let adapter: Box<dyn MagazineAdapter> = match source {
        MagazineSource::LondonReview => Box::new(LondonReviewAdapter),
        MagazineSource::Harpers => Box::new(HarpersAdapter),
    };
    let start_url = match source {
        MagazineSource::LondonReview => LRB_ARCHIVE_URL,
        MagazineSource::Harpers => HARPERS_ARCHIVE_URL,
    };
    let max_pages = max_archive_pages(&source);

    progress.step(&format!(
        "Discovering issues from {} (walking the whole archive)…",
        start_url
    ));
    let mut issues: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut visited_pages = std::collections::HashSet::new();
    let mut page = Some(start_url.to_string());
    let mut pages = 0usize;

    while let Some(url) = page {
        // Guard against pagination cycles (e.g. a "next" link pointing back at page 1).
        if !visited_pages.insert(url.clone()) {
            break;
        }
        let doc = fetch_html_body(&client, &url, &args.delay, progress)?;
        let mut found = 0;
        for issue in adapter.discover_issues(&doc, progress) {
            if seen.insert(issue.clone()) {
                issues.push(issue);
                found += 1;
            }
        }
        progress.info(&format!("{}: {} issues", url, found));

        pages += 1;
        if pages >= max_pages {
            progress.warn(&format!(
                "Stopped after {} archive pages (safety cap).",
                max_pages
            ));
            break;
        }
        page = adapter.next_archive_page(&doc, progress);
    }

    if issues.is_empty() {
        bail!("No issues found on the archive page. The site markup may have changed.");
    }

    let pending: Vec<&String> = issues
        .iter()
        .filter(|url| {
            let id = issue_id_from_url(url).unwrap_or_else(|| (*url).clone());
            issue_pending(args, history, source_code(&source), &id)
        })
        .collect();

    progress.info(&format!(
        "{} issues found, {} to download ({} already in history).",
        issues.len(),
        pending.len(),
        issues.len() - pending.len()
    ));

    if args.list {
        for url in &pending {
            println!("{}", url);
        }
        return Ok(());
    }

    let (mut downloaded, mut skipped, mut failed) = (0u32, 0u32, 0u32);
    for url in pending {
        progress.step(&format!("Issue {}", url));
        match process_issue(url, None, &source, &client, args, progress, history, history_path) {
            Ok(Outcome::Downloaded) => downloaded += 1,
            Ok(Outcome::Skipped) => skipped += 1,
            Err(e) => {
                failed += 1;
                progress.warn(&format!("Failed to download {}: {:#}", url, e));
            }
        }
    }

    progress.step(&format!(
        "Done. {} downloaded, {} skipped, {} failed.",
        downloaded, skipped, failed
    ));
    Ok(())
}

/// Download and build one issue, updating the history on success.
#[allow(clippy::too_many_arguments)]
fn process_issue(
    url: &str,
    name_override: Option<String>,
    source: &MagazineSource,
    client: &Client,
    args: &Args,
    progress: &Progress,
    history: &mut History,
    history_path: &Path,
) -> Result<Outcome> {
    let src_code = source_code(source);
    let issue_id = issue_id_from_url(url).unwrap_or_else(|| url.to_string());

    // Skip only if it's in history AND the file is still on disk (so a deleted EPUB is
    // regenerated). --force overrides everything.
    if !issue_pending(args, history, src_code, &issue_id) {
        progress.verbose(&format!("Skipping {} (already downloaded)", issue_id));
        return Ok(Outcome::Skipped);
    }

    let adapter: Box<dyn MagazineAdapter> = match source {
        MagazineSource::LondonReview => Box::new(LondonReviewAdapter),
        MagazineSource::Harpers => Box::new(HarpersAdapter),
    };

    progress.step("Fetching issue contents…");
    let doc = fetch_html_body(client, url, &args.delay, progress)?;
    let issue = adapter.extract_issue(&doc, progress);

    if issue.links.is_empty() {
        bail!("No articles found for this issue; the site markup may have changed.");
    }

    let filename = name_override
        .unwrap_or_else(|| format!("{} - {}", src_code, issue.title));
    let filename = sanitize_filename(&filename);
    let output_path = args.output.join(format!("{}.epub", filename));

    if !args.force && output_path.exists() {
        progress.info(&format!("{} already exists; skipping.", output_path.display()));
        // Record it so the history reflects reality on the next run.
        persist_download(
            history_path,
            history,
            HistoryEntry::now(src_code, &issue_id, &issue.title, format!("{}.epub", filename)),
        )?;
        return Ok(Outcome::Skipped);
    }

    progress.step(&format!(
        "Fetching {} articles ({} at a time)…",
        issue.links.len(),
        args.concurrency
    ));
    let rescue_from_archives = matches!(source, MagazineSource::Harpers);
    let mut articles = fetch_articles(
        client,
        &issue.links,
        args.delay,
        args.concurrency,
        progress,
        |d, article_url| {
            let article = adapter.extract_article(d, progress);
            // A suspiciously short body on a page carrying paywall markup means the
            // server truncated the article — try public archive snapshots.
            if rescue_from_archives && body_text_len(&article.body) < PAYWALL_SUSPECT_CHARS {
                try_archive_rescue(client, article_url, &*adapter, args.delay, progress, article)
            } else {
                article
            }
        },
    );

    if articles.is_empty() {
        bail!("Every article failed to download for this issue.");
    }

    // Drop sections with no usable content (an empty Index Archive card, a PDF-only
    // puzzle whose PDF we can't access) rather than shipping blank pages.
    articles.retain(|a| {
        let keep = body_text_len(&a.body) >= 40 || a.body.contains("<img");
        if !keep {
            progress.warn(&format!("Dropping empty article: {}", a.title));
        }
        keep
    });
    if articles.is_empty() {
        bail!("Every article in this issue was empty after extraction.");
    }

    // Ready-made sections extracted from the issue page itself (e.g. Harper's artwork
    // slideshow) go in after the fetched articles.
    articles.extend(issue.extra_articles);

    build_epub(
        progress,
        &issue.title,
        &issue.publication_name,
        &output_path,
        articles,
        &issue.cover_image_uri,
        client,
        url,
    )?;

    persist_download(
        history_path,
        history,
        HistoryEntry::now(src_code, &issue_id, &issue.title, format!("{}.epub", filename)),
    )?;

    Ok(Outcome::Downloaded)
}

/// Below this many characters of body text, a Harper's article is suspected to be a
/// paywall-truncated preview and public archives are tried. Genuinely short pieces
/// (poems, the puzzle) cost a couple of extra requests and keep their original text.
const PAYWALL_SUSPECT_CHARS: usize = 1500;

/// Plain-text length of an HTML body fragment (whitespace-trimmed).
fn body_text_len(body: &str) -> usize {
    let fragment = scraper::Html::parse_fragment(body);
    fragment
        .root_element()
        .text()
        .map(|t| t.trim().len())
        .sum()
}

/// Try to recover a paywalled/truncated article from public archives (Wayback Machine,
/// then archive.ph). A candidate replaces the original only if it yields strictly more
/// body text, so a genuinely short piece keeps its original content. Wayback preserves
/// the source markup (image URLs are rewritten to web.archive.org, which serve fine),
/// so the adapter's normal extraction applies.
fn try_archive_rescue(
    client: &Client,
    url: &str,
    adapter: &dyn MagazineAdapter,
    delay: u64,
    progress: &Progress,
    original: ArticleData,
) -> ArticleData {
    const ARCHIVE_PREFIXES: &[&str] = &[
        "https://web.archive.org/web/2/",
        "https://archive.ph/newest/",
    ];

    let mut best = original;
    let mut best_len = body_text_len(&best.body);
    progress.warn(&format!(
        "{} looks paywalled/truncated ({} chars); trying public archives…",
        url, best_len
    ));

    for prefix in ARCHIVE_PREFIXES {
        let archive_url = format!("{}{}", prefix, url);
        match fetch_html_body(client, &archive_url, &delay, progress) {
            Ok(doc) => {
                let mut cand = adapter.extract_article(&doc, progress);
                let len = body_text_len(&cand.body);
                if len > best_len {
                    // The archive copy sometimes garbles metadata; keep the original's
                    // title/byline where the candidate's are missing.
                    if cand.title == "Untitled" || cand.title.is_empty() {
                        cand.title = best.title.clone();
                    }
                    cand.byline = cand.byline.or_else(|| best.byline.clone());
                    progress.info(&format!(
                        "Recovered {} from {} ({} chars)",
                        cand.title, prefix, len
                    ));
                    best = cand;
                    best_len = len;
                }
                if best_len >= PAYWALL_SUSPECT_CHARS {
                    break;
                }
            }
            Err(e) => {
                progress.verbose(&format!("Archive fetch failed {}: {:#}", archive_url, e));
            }
        }
    }
    best
}

/// Whether an issue still needs downloading: forced, absent from history, or its recorded
/// EPUB file has since been deleted from the output directory.
fn issue_pending(args: &Args, history: &History, src_code: &str, issue_id: &str) -> bool {
    if args.force {
        return true;
    }
    match history.get(src_code, issue_id) {
        Some(entry) => !args.output.join(&entry.filename).exists(),
        None => true,
    }
}

/// Record a completed download and persist it, merging with the on-disk history first so a
/// concurrent run's entries aren't clobbered. Keeps the in-memory history in sync with disk.
fn persist_download(path: &Path, history: &mut History, entry: HistoryEntry) -> Result<()> {
    let mut disk = History::load(path).unwrap_or_default();
    disk.record(entry);
    disk.save(path)?;
    *history = disk;
    Ok(())
}

/// Optional subscriber cookies for Harper's (raw Cookie header via HARPERS_COOKIE).
///
/// Running without cookies is normal and usually preferable: the metered paywall counts
/// reads via cookies, and this client has no cookie jar, so every request arrives like a
/// fresh private-browsing window and the meter never accumulates.
fn harpers_cookie_for(source: &MagazineSource) -> Option<String> {
    if !matches!(source, MagazineSource::Harpers) {
        return None;
    }
    match std::env::var("HARPERS_COOKIE") {
        Ok(c) if !c.is_empty() => Some(c),
        _ => None,
    }
}

/// Make a title safe to use as a filename across common filesystems.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "issue".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{body_text_len, sanitize_filename};

    #[test]
    fn test_body_text_len_counts_text_not_markup() {
        assert_eq!(body_text_len("<p>hi</p>"), 2);
        assert_eq!(body_text_len("<div><img src=\"x.jpg\"/></div>"), 0);
        assert!(body_text_len("<p>one two three</p>") >= 11);
    }

    #[test]
    fn test_sanitize_strips_path_separators() {
        assert_eq!(sanitize_filename("a/b\\c"), "a-b-c");
    }

    #[test]
    fn test_sanitize_keeps_interpunct() {
        assert_eq!(
            sanitize_filename("Vol. 48 No. 1 · 2 January 2026"),
            "Vol. 48 No. 1 · 2 January 2026"
        );
    }

    #[test]
    fn test_sanitize_traversal() {
        // No path separators survive, so join() can't escape the output directory.
        assert!(!sanitize_filename("../../etc/passwd").contains('/'));
    }

    #[test]
    fn test_sanitize_empty_fallback() {
        assert_eq!(sanitize_filename("   "), "issue");
    }
}
