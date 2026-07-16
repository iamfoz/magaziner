//! Curated, e-reader-optimised CSS for generated EPUBs.
//!
//! `magaziner` scrapes article HTML from source websites and re-packages it
//! into EPUB content documents. The scraped pages carry their own site CSS,
//! but shipping that CSS verbatim into the EPUB produces poor results on
//! e-ink readers (Kindle, Kobo, etc.): fixed-width layouts, web fonts that
//! don't exist on the device, low-contrast colours that fight the reader's
//! own light/dark theme, and typography tuned for a backlit screen rather
//! than a page-turning device.
//!
//! This module replaces that behaviour with a single, hand-tuned stylesheet
//! designed from scratch for reflowable e-reader content:
//!
//! - A comfortable serif reading font stack with generous line-height,
//!   justified text, and hyphenation hints.
//! - Sensible paragraph indentation (block-style: no indent on the first
//!   paragraph after a heading, indent on subsequent paragraphs).
//! - A clear, restrained heading hierarchy with page-break-friendly rules.
//! - Distinct, readable treatment for bylines, reviewed-item lists,
//!   blockquotes, figures/captions, and the table of contents.
//! - No explicit body foreground/background colours, no web fonts, no
//!   `@import`, and no flexbox/grid — only CSS that e-ink rendering engines
//!   are known to support well, so the reader's own theme (including dark
//!   mode) is always respected.
//!
//! The stylesheet is embedded as a Rust string constant and written into
//! the EPUB as `stylesheet.css`, linked from every content document.

/// The main EPUB stylesheet, linked from every content document as "stylesheet.css".
pub const STYLESHEET: &str = r#"/* =====================================================================
   magaziner e-reader stylesheet
   Designed for reflowable EPUB rendering on e-ink devices (Kindle, Kobo,
   and similar). Deliberately avoids explicit body colours, web fonts,
   @import, and flexbox/grid layout so it degrades gracefully across
   reading systems and respects the reader's own light/dark theme.
   ===================================================================== */

/* ---------------------------------------------------------------------
   Base typography
   --------------------------------------------------------------------- */

body {
  font-family: Georgia, "Palatino Linotype", "Book Antiqua", Palatino, "Times New Roman", serif;
  line-height: 1.5;
  text-align: justify;
  -webkit-hyphens: auto;
  -moz-hyphens: auto;
  -ms-hyphens: auto;
  hyphens: auto;
  margin: 0 5%;
  orphans: 2;
  widows: 2;
}

p {
  margin: 0;
  text-indent: 1.25em;
  orphans: 2;
  widows: 2;
}

/* First paragraph after a heading or the reviewed-items block should not
   be indented, matching conventional book typesetting. */
h1 + p,
h2 + p,
h3 + p,
.article-title + p,
.byline + p,
.reviewed-items + p {
  text-indent: 0;
}

/* A subtle, optional drop-cap for the article's opening paragraph. Many
   reading systems ignore ::first-letter entirely, which is fine -- the
   paragraph still reads correctly without it. */
.article-title + p::first-letter,
.byline + p::first-letter {
  font-size: 2.4em;
  line-height: 0.9;
  font-weight: bold;
  float: left;
  padding-right: 0.08em;
  padding-top: 0.02em;
}

/* ---------------------------------------------------------------------
   Headings
   --------------------------------------------------------------------- */

h1,
h2,
h3,
h4,
h5,
h6 {
  font-family: Georgia, "Palatino Linotype", "Book Antiqua", Palatino, "Times New Roman", serif;
  font-weight: bold;
  text-align: left;
  text-indent: 0;
  line-height: 1.25;
  page-break-after: avoid;
  page-break-inside: avoid;
  -webkit-hyphens: none;
  hyphens: none;
}

h1 {
  font-size: 1.6em;
  margin: 0 0 0.6em 0;
}

h2 {
  font-size: 1.3em;
  margin: 1.4em 0 0.5em 0;
  border-top: 1px solid currentColor;
  padding-top: 0.6em;
}

h3 {
  font-size: 1.1em;
  font-style: italic;
  font-weight: normal;
  margin: 1.2em 0 0.4em 0;
}

h4, h5, h6 {
  font-size: 1em;
  margin: 1em 0 0.3em 0;
}

/* ---------------------------------------------------------------------
   Title page (issue title + publication name)
   --------------------------------------------------------------------- */

.title-page,
.titlepage {
  text-align: center;
}

.title-page h1,
.titlepage h1 {
  text-align: center;
  font-size: 1.8em;
  margin: 2em 0 0.4em 0;
}

.title-page h3,
.titlepage h3 {
  text-align: center;
  font-style: italic;
  font-weight: normal;
  font-size: 1.1em;
  margin: 0 0 2em 0;
  page-break-after: avoid;
}

/* ---------------------------------------------------------------------
   Article title & byline
   --------------------------------------------------------------------- */

.article-title {
  text-align: left;
  font-size: 1.7em;
  line-height: 1.25;
  margin: 0.2em 0 0.3em 0;
  padding-bottom: 0.4em;
  border-bottom: 1px solid currentColor;
  page-break-after: avoid;
  text-indent: 0;
  -webkit-hyphens: none;
  hyphens: none;
}

.byline {
  text-align: left;
  font-style: italic;
  font-variant: small-caps;
  font-size: 0.9em;
  letter-spacing: 0.02em;
  margin: 0 0 1.2em 0;
  text-indent: 0;
  page-break-after: avoid;
  opacity: 0.85;
}

/* ---------------------------------------------------------------------
   Reviewed items list
   A visually distinct block listing the books/works under review.
   Uses only borders and font styling (no fixed background fill) so it
   stays legible against both light and dark e-reader themes.
   --------------------------------------------------------------------- */

.reviewed-items {
  margin: 1em 0 1.5em 0;
  padding: 0.75em 1em;
  border: 1px solid currentColor;
  border-left-width: 3px;
  font-size: 0.9em;
  line-height: 1.4;
  font-style: italic;
  text-align: left;
  text-indent: 0;
  page-break-inside: avoid;
}

.reviewed-items p {
  text-indent: 0;
  margin: 0.2em 0;
}

.reviewed-items ul,
.reviewed-items ol {
  margin: 0.2em 0;
  padding-left: 1.4em;
}

.reviewed-items li {
  margin: 0.2em 0;
}

/* ---------------------------------------------------------------------
   Blockquotes
   --------------------------------------------------------------------- */

blockquote {
  margin: 1em 1.5em;
  padding-left: 0.9em;
  border-left: 3px solid currentColor;
  font-style: italic;
  text-indent: 0;
  page-break-inside: avoid;
}

blockquote p {
  text-indent: 0;
  margin: 0.4em 0;
}

/* ---------------------------------------------------------------------
   Inline emphasis
   --------------------------------------------------------------------- */

em, i {
  font-style: italic;
}

strong, b {
  font-weight: bold;
}

/* ---------------------------------------------------------------------
   Horizontal rules (section breaks)
   --------------------------------------------------------------------- */

hr {
  width: 30%;
  margin: 1.6em auto;
  border: none;
  border-top: 1px solid currentColor;
  opacity: 0.5;
}

/* ---------------------------------------------------------------------
   Figures / images
   --------------------------------------------------------------------- */

figure {
  margin: 1.4em 0;
  text-align: center;
  page-break-inside: avoid;
}

img {
  max-width: 100%;
  height: auto;
}

figcaption {
  font-size: 0.85em;
  font-style: italic;
  text-align: center;
  text-indent: 0;
  margin-top: 0.5em;
  opacity: 0.85;
}

/* ---------------------------------------------------------------------
   Table of contents
   --------------------------------------------------------------------- */

nav[epub|type~="toc"] ol,
.toc ol,
.contents ol {
  list-style-type: none;
  padding-left: 0;
  margin: 0.5em 0;
}

nav[epub|type~="toc"] li,
.toc li,
.contents li {
  margin: 0.7em 0;
  line-height: 1.4;
}

nav[epub|type~="toc"] a,
.toc a,
.contents a {
  text-decoration: none;
}

nav[epub|type~="toc"] h2,
.toc h2,
.contents h2 {
  border-top: none;
  padding-top: 0;
  text-align: center;
}

/* ---------------------------------------------------------------------
   Misc
   --------------------------------------------------------------------- */

a {
  text-decoration: none;
}
"#;
