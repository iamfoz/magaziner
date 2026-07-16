# PRD: magaziner — LRB EPUB Quality & Automation Enhancements

Status: Implemented — build/clippy/tests green; EPUB output validated offline
Owner: iamfoz
Date: 2026-07-16

## 1. Background

`magaziner` is a Rust CLI that downloads a magazine issue (London Review of Books,
Harper's Magazine) and produces an EPUB for e-readers. Real-world LRB output is
"lacklustre": covers often don't appear, styling is crude, article images are
stripped, and there is no automation for keeping a library up to date.

This PRD captures the full scope of enhancements and the bugs to fix, and drives
the implementation.

## 2. Goals

1. **Reliable cover download** — fetch the real LRB issue cover and embed it as the
   EPUB cover image, handling relative URLs and lazy-load attributes.
2. **Advanced, readable styling** — replace the wholesale dump of site CSS with a
   curated, e-reader-optimised stylesheet; improve title page, TOC, and article
   typography (drop caps, block quotes, reviewed-items styling, bylines, etc.).
3. **Automated `--all` mode** — discover every issue from the LRB archive and
   download any that are not yet in the history file.
4. **Download history** — a persistent history file records downloaded issues and
   prevents re-downloading, unless `--force` is passed.
5. **Bug fixes** — resolve the correctness, robustness, and validity bugs found in
   the audit (see §5).
6. **Additional quality features** — see §6.

## 3. Non-goals

- No changes to the Harper's authentication model beyond bug fixes.
- No GUI. Remains a CLI.
- No parallel/concurrent fetching (politeness to the source is preferred).

## 4. Feature specifications

### 4.1 Cover download (LRB)
- Extract the cover URL from `div.article-issue-cover-image img`, preferring
  `data-appsrc`, then the first `srcset` candidate, then `src`.
- **Resolve relative URLs to absolute** against `https://www.lrb.co.uk` (root bug:
  LRB serves `/storage/...` relative paths that are currently dropped).
- Download with correct `Content-Type` detection (jpeg/png/webp) rather than a
  hardcoded `image/jpeg`.
- Write the temp cover into a temp directory (not CWD), and always clean up.
- If no cover is found, warn and continue (no panic).

### 4.2 Advanced styling
- Curated stylesheet embedded as a Rust constant (`src/style.rs`), applied as the
  EPUB stylesheet instead of scraped site CSS.
- Typographic treatment for: issue/title page, TOC, article title + byline,
  `reviewed-items` block, block quotes, drop cap on first paragraph (optional/subtle),
  footnotes/small print, horizontal rules, images with captions.
- Light/dark friendly (respect reader defaults; avoid hardcoded backgrounds).
- Each article XHTML links the shared stylesheet rather than embedding an inline
  `<style>` block.

### 4.3 `--all` automated mode
- New flag `--all`. When set, `--url` is optional.
- Discover issue URLs from the LRB archive listing (`https://www.lrb.co.uk/the-paper`
  and paginated back-issue listing), producing canonical `.../vNN/nNN` URLs.
- Filter out issues already present in the history file (unless `--force`).
- Download each remaining issue sequentially, honouring `--delay`.
- Robust: a failure on one issue is logged and does not abort the run; a summary is
  printed at the end (N downloaded, M skipped, K failed).

### 4.4 Download history
- History file (JSON) at a stable location; default `<output>/.magaziner-history.json`,
  overridable with `--history <PATH>`.
- Records: source, issue identifier (e.g. `v48/n01`), issue title, output filename,
  ISO 8601 timestamp.
- Before downloading, consult history; skip if present unless `--force`.
- After a successful download, append/update the record. Written atomically.

### 4.5 CLI surface (new/changed flags)
- `--all` — download all undownloaded issues (LRB).
- `--history <PATH>` — path to history file.
- `--url` becomes optional when `--all` is used (mutually satisfying).
- Existing flags retained: `--output`, `--delay`, `--force`, `--verbose`, `--quiet`, `--name`.

## 5. Bugs to fix (from audit)

Confirmed from code reading.

- **B1 (critical):** `london_review_adapter.rs:38` `.next().unwrap()` panics when no
  cover image element exists.
- **B2 (critical/core):** Cover URLs are not resolved to absolute; relative
  `/storage/...` URLs are then dropped by `epub.rs` `image_uri.starts_with("http")`,
  so covers silently never embed.
- **B3 (high):** Article/issue titles and TOC entries are inserted into XHTML
  unescaped — a `&`, `<`, or `>` in a title yields invalid XHTML.
- **B4 (high):** `sanitize_html_for_epub` only maps a handful of named HTML entities
  to numeric; any other named entity (e.g. `&eacute;`, `&pound;`, `&copy;`) is
  invalid in XHTML and can make the EPUB fail validation.
- **B5 (high):** All `<img>` tags are stripped from article bodies, removing
  in-article figures/artwork → lacklustre output. Should optionally embed images.
- **B6 (medium):** A single failed article fetch aborts the entire issue via `?`.
  Should log and skip.
- **B7 (medium):** Output filename is built from the issue title without sanitising
  filesystem-hostile characters (`/`, etc.).
- **B8 (medium):** Cover temp file written to CWD as `cover.jpg`; not cleaned up on
  error; hardcoded content type.
- **B9 (low):** `sanitize_html_for_epub` contains a no-op `.replace("<img ", "<img ")`.
- **B10 (low):** `progress.rs` total steps hardcoded to 5 and `substep` reuses the
  step counter oddly; needs to accommodate `--all` / variable pipelines.
- **B11 (low):** LRB article title retains the site suffix (e.g. `· London Review of
  Books` is fine, but leading/trailing noise should be trimmed).

## 6. Additional features (proposed)

- **F1:** Per-article metadata — capture author/byline where available and render it.
- **F2:** EPUB metadata — set language, publication date, unique identifier, and
  publisher.
- **F3:** Fetch retry with backoff (transient network errors) in `fetch.rs`.
- **F4:** `--list` / dry-run for `--all` — print which issues would be downloaded.
- **F5:** Embed in-article images (download + rewrite `src`) for true fidelity (ties
  to B5); guarded so failures degrade gracefully.
- **F6 (added mid-build, user request):** Concurrent article downloads after the issue
  page is fetched, via a bounded worker pool (`--concurrency`, default 4). Order is
  preserved; per-request `--delay` is still honoured for politeness.
- **F7 (added mid-build, user request):** Smooth in-place progress bar (Unicode
  block-eighths resolution) during article fetching, thread-safe for the concurrent pool.

## 6b. Implementation notes

- All §5 bugs B1–B11 fixed, plus audit items #4 (HTTP status via `error_for_status`),
  #7 (regex img fragility → DOM serializer), #8 (stylesheet now linked from every content
  doc), #9 (link absolutization), #11 (Harper's readings fallback), #12 (Harper's fallback
  title suffix), #14 (client timeout), #18 (atomic EPUB write).
- Cover download and image embedding go straight to memory (no CWD temp file), with real
  MIME detection.
- Article bodies are re-serialized from a parsed DOM (`ego-tree`) rather than string-munged,
  guaranteeing valid XHTML (verified: every content doc parses as XML).

## 7. Work breakdown

| Workstream | Scope |
|---|---|
| Audit | Read-only bug sweep cross-checking §5 |
| WS1: history module | New `src/history.rs` + unit tests |
| WS2: styling module | New `src/style.rs` curated CSS + wiring |
| WS3: core | Adapter cover/absolute-URL, archive discovery, EPUB styling + escaping + images, main pipeline refactor, `--all`, history wiring, CLI |

New modules are self-contained files to keep the changes reviewable; each workstream
ends with build, clippy, tests, and an end-to-end EPUB sanity check.

## 8. Acceptance criteria

- `cargo build`, `cargo clippy`, and `cargo test` are clean.
- Cover extraction resolves relative LRB URLs to absolute and unit tests prove it.
- Generated article/TOC/title XHTML is well-formed even with `&`/`<`/`>` in titles
  (unit tests for escaping).
- History file round-trips: written after download, respected on re-run, bypassed by
  `--force` (unit tests).
- `--all` discovers issue URLs from an archive fixture and filters against history
  (unit tests on discovery + filtering).
- Curated stylesheet is applied instead of scraped site CSS.
- README updated to document new flags and behaviour.
- All new logic covered by unit tests using local fixtures (no live network in tests).

## 9. Testing strategy

- Extend `src/test/lrb/` with fixtures: a realistic issue page with a relative
  cover URL, and an archive-listing page for `--all` discovery.
- Unit tests per module; no network calls in tests.
- Manual/one-off: build release binary and run `--help` to confirm CLI wiring.
  (Live LRB fetch is blocked by the sandbox egress policy, so end-to-end network
  runs are validated by the user in their environment.)
