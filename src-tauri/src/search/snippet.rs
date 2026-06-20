//! Highlighted snippet generation for search results.
//!
//! Wraps Tantivy's `SnippetGenerator`, which finds the best-matching
//! fragment of a document's stored `content` and emits HTML with matched
//! terms wrapped in `<b>...</b>`. When the query matched only non-content
//! fields (e.g. the filename), we fall back to the start of the content.

use anyhow::Result;
use tantivy::query::Query;
use tantivy::schema::{Field, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::{Searcher, TantivyDocument};

/// Maximum number of characters in a generated snippet.
const MAX_SNIPPET_CHARS: usize = 220;

pub struct SnippetMaker {
    generator: SnippetGenerator,
    content: Field,
    max_chars: usize,
}

impl SnippetMaker {
    /// Build a snippet generator for `content` against the given query.
    pub fn new(searcher: &Searcher, query: &dyn Query, content: Field) -> Result<Self> {
        let mut generator = SnippetGenerator::create(searcher, query, content)?;
        generator.set_max_num_chars(MAX_SNIPPET_CHARS);
        Ok(Self {
            generator,
            content,
            max_chars: MAX_SNIPPET_CHARS,
        })
    }

    /// Produce an HTML snippet for `doc`. Matched terms are wrapped in `<b>`.
    pub fn make(&self, doc: &TantivyDocument) -> String {
        let html = self.generator.snippet_from_doc(doc).to_html();
        if !html.is_empty() {
            return html;
        }
        // No content match: show the beginning of the file (escaped).
        let text = doc
            .get_first(self.content)
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let truncated: String = text.trim().chars().take(self.max_chars).collect();
        escape_html(&truncated)
    }
}

/// Minimal HTML escaping for the non-highlighted fallback path.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
