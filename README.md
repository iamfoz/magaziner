# magaziner

A fast, self-hosted CLI tool that downloads a magazine issue from your subscription and generates a clean, portable **EPUB** file — ready to load on any e-reader.

Currently supports:
- **London Review of Books** (`lrb.co.uk`)
- **Harper's Magazine** (`harpers.org`)

---

## Why

Magazine websites are noisy, require a live internet connection, and often restrict offline access. `magaziner` fetches every article in an issue, strips navigation and ads, and bundles the result into a standards-compliant EPUB. Read your subscription on a Kindle, Kobo, or any e-reader app — without a browser.

---

## Features

- Generates a complete EPUB from a single issue URL
- Fetches the real cover image (resolving lazy-loaded, relative URLs) and embeds it with the correct MIME type
- Embeds in-article images as EPUB resources rather than stripping them
- Ships a curated, e-reader-optimised stylesheet — clean typography, styled block quotes, reviewed-items, bylines, and figures — instead of dumping the website's own CSS
- **Automated `--all` mode**: discovers LRB issues from the archive and downloads any not already fetched
- **Download history**: records every downloaded issue and skips re-downloads unless `--force`
- **Concurrent article downloads** with a configurable worker count (`--concurrency`)
- A smooth in-place progress bar
- Robust fetching: browser User-Agent, request timeouts, HTTP-error detection, and automatic retry with exponential backoff
- Skips (rather than aborts on) an individual article that fails to download
- Valid, well-formed XHTML output: text is XML-escaped, void elements self-closed, scripts/iframes stripped
- Builds a linked table of contents
- Configurable per-request delay for polite rate limiting
- Verbose and quiet output modes for scripting
- Adapter-based architecture — adding a new publication is self-contained
- Authenticated fetching via cookie passthrough (Harper's)

---

## Installation

### Homebrew (macOS and Linux)

```bash
brew install colbsmcdolbs/tap/magaziner
```

### cargo (any platform with Rust)

```bash
cargo install magaziner
```

### Build from source

```bash
git clone https://github.com/colbsmcdolbs/magaziner
cd magaziner
cargo build --release
```

The binary will be at `./target/release/magaziner`. You can copy it anywhere on your `$PATH`:

```bash
cp target/release/magaziner ~/.local/bin/
```

---

## Usage

```
magaziner [OPTIONS]

Options:
  -u, --url <URL>                  Magazine archive URL (LRB or Harper's)
      --all                        Download every LRB issue not already in the history file
      --list                       With --all: list the issues that would be downloaded, then exit
  -o, --output <OUTPUT>            Output directory for generated EPUBs [default: .]
      --history <HISTORY>          Path to the download-history file [default: <output>/.magaziner-history.json]
  -d, --delay <DELAY>              Delay before each request, in milliseconds [default: 3000]
  -c, --concurrency <CONCURRENCY>  Number of concurrent article downloads [default: 4]
  -f, --force                      Re-download even if the issue is already in the history
  -v, --verbose                    Print detailed network and parsing logs
  -q, --quiet                      Suppress all output (for scripting)
  -n, --name <NAME>                Custom output filename without extension (single-issue mode)
  -h, --help                       Print help
  -V, --version                    Print version
```

Either `--url` or `--all` is required.

### Automated mode & history

`--all` discovers issues from the LRB archive index and downloads every one that is not
already recorded in the history file:

```bash
magaziner --all --output ~/Books
```

Each successful download is recorded in `~/Books/.magaziner-history.json` (override with
`--history`). On the next run, issues already in the history are skipped, so `--all` only
fetches what's new. Use `--force` to re-download regardless of history, or `--list` to see
which issues *would* be downloaded without fetching anything:

```bash
magaziner --all --list          # dry run
magaziner --all --force         # re-download everything discovered
```

> Discovery reads the issues linked from the archive index page. To backfill a specific
> older issue that isn't linked there, download it directly by URL.

### Concurrency

After the issue's table of contents is fetched, articles are downloaded concurrently. Tune
the number of simultaneous requests with `--concurrency` (default `4`); each request still
waits `--delay` milliseconds first, to stay polite to the source:

```bash
magaziner --url https://www.lrb.co.uk/the-paper/v48/n01 --concurrency 6 --delay 2000
```

### London Review of Books

```bash
magaziner --url https://www.lrb.co.uk/the-paper/v47/n06
```

The URL must match the format `https://www.lrb.co.uk/the-paper/vNN/nNN` — the issue index page, not an individual article.

### Harper's Magazine

Harper's requires an active subscription to access full article content. Export your session cookie from a logged-in browser and set the `HARPERS_COOKIE` environment variable before running:

```bash
export HARPERS_COOKIE="your_session_cookie_string_here"
magaziner --url https://harpers.org/archive/2026/02
```

The URL must match the format `https://harpers.org/archive/YYYY/MM`.

> **Getting your cookie:** In Chrome or Firefox, open DevTools → Application → Cookies while logged in to `harpers.org`, then copy the full cookie string from the `Cookie` request header (visible in the Network tab on any page request).

---

## Examples

Download an LRB issue to the current directory:

```bash
magaziner --url https://www.lrb.co.uk/the-paper/v47/n06
```

Download to a specific folder, overwriting if it exists:

```bash
magaziner --url https://www.lrb.co.uk/the-paper/v47/n06 --output ~/Books --force
```

Download a Harper's issue with a faster request cadence and verbose logging:

```bash
HARPERS_COOKIE="..." magaziner \
  --url https://harpers.org/archive/2026/02 \
  --output ~/Books \
  --delay 1000 \
  --verbose
```

Use in a shell script (quiet mode, exits non-zero on error):

```bash
HARPERS_COOKIE="..." magaziner \
  --url https://harpers.org/archive/2026/02 \
  --output ~/Books \
  --quiet
```

---

## Output

The generated file is named after the issue and written to the output directory:

```
~/Books/February 2026.epub
~/Books/Vol. 47 No. 6 · 20 March 2025.epub
```

The EPUB includes:
- A cover image (fetched from the issue page)
- A title page
- A linked table of contents
- All articles, formatted for e-readers

---

## Supported URL Formats

| Publication | URL Format | Example |
|---|---|---|
| London Review of Books | `https://www.lrb.co.uk/the-paper/vNN/nNN` | `https://www.lrb.co.uk/the-paper/v47/n06` |
| Harper's Magazine | `https://harpers.org/archive/YYYY/MM` | `https://harpers.org/archive/2026/02` |

`magaziner` validates the URL at startup and will clearly tell you what format is expected if it doesn't match.

---

## Architecture

```
src/
├── main.rs                   # CLI args (clap), single-issue + --all pipeline
├── adapter.rs                # MagazineAdapter trait + IssueData/ArticleData structs
├── london_review_adapter.rs  # LRB HTML parsing + archive discovery
├── harpers_adapter.rs        # Harper's HTML parsing
├── fetch.rs                  # HTTP client, retry/backoff, concurrent article fetch
├── epub.rs                   # EPUB assembly, DOM→XHTML serialization, image embedding
├── style.rs                  # Curated e-reader stylesheet (embedded constant)
├── history.rs                # Download-history store (atomic JSON)
├── validation.rs             # URL validation, source detection, URL helpers
└── progress.rs               # Progress output + in-place progress bar
```

### Pipeline

1. **Resolve targets** — a single validated `--url`, or (`--all`) issue URLs discovered from
   the LRB archive index, filtered against the download history
2. **Build HTTP client** — `reqwest::blocking::Client` with a browser User-Agent, timeout,
   and optional `Cookie` header
3. **Fetch issue page** — parse article links, title, and the (absolutized) cover image URL
4. **Fetch articles concurrently** — bounded worker pool, retry with backoff, per-request
   delay; a failed article is skipped, not fatal
5. **Build EPUB** — re-serialize each article body from a parsed DOM into valid XHTML,
   embed the cover and in-article images, apply the curated stylesheet, assemble with TOC,
   and write atomically
6. **Record history** — append the issue to the history file so it isn't re-downloaded

### Adding a new publication

Implement `MagazineAdapter` for your new source:

```rust
pub trait MagazineAdapter: Sync {
    fn extract_issue(&self, doc: &Html, progress: &Progress) -> IssueData;
    fn extract_article(&self, doc: &Html, progress: &Progress) -> ArticleData;

    // Optional: parse an archive listing into issue URLs for `--all`.
    // Defaults to returning no issues.
    fn discover_issues(&self, doc: &Html, progress: &Progress) -> Vec<String> { Vec::new() }
}
```

(The `Sync` bound lets a single stateless adapter drive concurrent article fetches.)

Then add a regex branch to `detect_source()` in `validation.rs` and wire up the adapter in `main.rs`. No other files need to change.

---

## Environment Variables

| Variable | Description |
|---|---|
| `HARPERS_COOKIE` | Raw `Cookie` header value for an authenticated Harper's session. Required for full subscriber access. If unset, a warning is printed and only free-tier content will be available. |

---

## Development

```bash
# Run all tests
cargo test

# Run a specific test
cargo test test_extract_article_links_from_harpers_issue

# Lint
cargo clippy

# Build optimized release binary
cargo build --release
```

### Test fixtures

Integration-style tests use saved HTML fixture files rather than live network requests:

```
src/test/
├── lrb/
│   ├── issue.html    # LRB issue index page
│   └── article.html  # LRB article page
└── harpers/
    ├── issue.html    # Harper's issue index page
    └── article.html  # Harper's article page
```

---

## Dependencies

| Crate | Purpose |
|---|---|
| [`clap`](https://crates.io/crates/clap) | CLI argument parsing |
| [`reqwest`](https://crates.io/crates/reqwest) | Blocking HTTP client |
| [`scraper`](https://crates.io/crates/scraper) | HTML parsing via CSS selectors |
| [`epub-builder`](https://crates.io/crates/epub-builder) | EPUB file generation |
| [`ego-tree`](https://crates.io/crates/ego-tree) | DOM traversal for XHTML re-serialization |
| [`regex`](https://crates.io/crates/regex) | URL validation |
| [`url`](https://crates.io/crates/url) | URL parsing and resolution |
| [`serde`](https://crates.io/crates/serde) + [`serde_json`](https://crates.io/crates/serde_json) | Download-history serialization |
| [`chrono`](https://crates.io/crates/chrono) | History timestamps |
| [`anyhow`](https://crates.io/crates/anyhow) | Ergonomic error handling |

---

## Legal

This tool is intended for personal use with content you have a valid subscription to access. Downloading and archiving articles for personal offline reading is consistent with fair use principles in many jurisdictions, but you are responsible for complying with each publication's terms of service and applicable law in your country.
