use crate::progress::Progress;
use scraper::Html;

pub struct IssueData {
    pub links: Vec<String>,
    pub title: String,
    pub cover_image_uri: String,
    pub publication_name: String,
}

pub struct ArticleData {
    pub title: String,
    pub byline: Option<String>,
    pub body: String,
}

/// A source-specific parser. Implementations are stateless (zero-sized) and therefore
/// `Sync`, so a single adapter can drive concurrent article fetches across threads.
pub trait MagazineAdapter: Sync {
    fn extract_issue(&self, doc: &Html, progress: &Progress) -> IssueData;
    fn extract_article(&self, doc: &Html, progress: &Progress) -> ArticleData;

    /// Parse an archive/back-issues listing page into canonical issue URLs.
    /// Sources that don't support automated discovery return an empty list.
    fn discover_issues(&self, _doc: &Html, _progress: &Progress) -> Vec<String> {
        Vec::new()
    }

    /// Given an archive page, return the URL of the next (older) archive page to visit,
    /// enabling discovery to walk back through every volume. `None` ends the walk.
    fn next_archive_page(&self, _doc: &Html, _progress: &Progress) -> Option<String> {
        None
    }
}
