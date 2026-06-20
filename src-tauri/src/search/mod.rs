//! Search core: parse a query, run it against an index, and return hits
//! with highlighted snippets.
//!
//! Milestone 3 covers the Tantivy `QueryParser` path, which natively
//! supports boolean (`AND`/`OR`/`NOT`, `+`/`-`), phrase (`"..."`) and
//! proximity (`"a b"~N`) syntax. Wildcard/regex/fuzzy come in Milestone 4.

pub mod query;
pub mod snippet;

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{Index, IndexReader, TantivyDocument};

use crate::index::schema::{build_schema, Fields};
use query::QueryBuilder;
use snippet::SnippetMaker;

/// A single search result, ready to be serialized to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub filename: String,
    pub ext: String,
    pub size: u64,
    /// Modified time as unix seconds.
    pub modified: i64,
    pub score: f32,
    /// HTML snippet with matched terms wrapped in `<b>`.
    pub snippet: String,
}

/// An open index ready to be queried. Cheap to clone via a fresh reader.
pub struct SearchEngine {
    reader: IndexReader,
    fields: Fields,
    query_parser: QueryParser,
}

impl SearchEngine {
    /// Open the index at `index_dir` for searching.
    pub fn open(index_dir: &Path) -> Result<Self> {
        let (_schema, fields) = build_schema();
        let index = Index::open_in_dir(index_dir)
            .with_context(|| format!("opening index at {}", index_dir.display()))?;
        let reader = index.reader().context("creating index reader")?;
        // Bare terms search content and filename by default.
        let query_parser = QueryParser::for_index(&index, vec![fields.content, fields.filename]);
        Ok(Self {
            reader,
            fields,
            query_parser,
        })
    }

    /// Run `query_str` and return up to `limit` hits ordered by score.
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let query = QueryBuilder::new(&self.query_parser, self.fields.content)
            .build(query_str)
            .with_context(|| format!("parsing query {query_str:?}"))?;

        let searcher = self.reader.searcher();
        let top = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;

        let snippet_maker = SnippetMaker::new(&searcher, query.as_ref(), self.fields.content)?;

        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            let get_str = |field| {
                doc.get_first(field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            hits.push(SearchHit {
                path: get_str(self.fields.path),
                filename: get_str(self.fields.filename),
                ext: get_str(self.fields.ext),
                size: doc
                    .get_first(self.fields.size)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                modified: doc
                    .get_first(self.fields.modified)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                score,
                snippet: snippet_maker.make(&doc),
            });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{build_index, BuildOptions};
    use std::fs;
    use tempfile::TempDir;

    /// Build a fixture index and return (kept-alive temp dirs, engine).
    fn fixture() -> (TempDir, TempDir, SearchEngine) {
        let src = tempfile::tempdir().unwrap();
        let idx = tempfile::tempdir().unwrap();
        fs::write(src.path().join("file1.txt"), b"the quick brown fox jumps").unwrap();
        fs::write(src.path().join("file2.txt"), b"a lazy dog sleeps").unwrap();
        fs::write(src.path().join("file3.txt"), b"quick dog runs fast").unwrap();

        build_index(
            idx.path(),
            &[src.path().to_path_buf()],
            &BuildOptions::default(),
            |_| {},
        )
        .unwrap();

        let engine = SearchEngine::open(idx.path()).unwrap();
        (src, idx, engine)
    }

    fn names(hits: &[SearchHit]) -> Vec<String> {
        let mut v: Vec<String> = hits.iter().map(|h| h.filename.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn single_term() {
        let (_s, _i, engine) = fixture();
        let hits = engine.search("fox", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt"]);
    }

    #[test]
    fn boolean_and() {
        let (_s, _i, engine) = fixture();
        let hits = engine.search("quick AND dog", 10).unwrap();
        assert_eq!(names(&hits), vec!["file3.txt"]);
    }

    #[test]
    fn boolean_or() {
        let (_s, _i, engine) = fixture();
        let hits = engine.search("quick OR lazy", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt", "file2.txt", "file3.txt"]);
    }

    #[test]
    fn boolean_not() {
        let (_s, _i, engine) = fixture();
        // quick but not dog -> file1 only (file3 has both quick and dog).
        let hits = engine.search("quick -dog", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt"]);
    }

    #[test]
    fn phrase() {
        let (_s, _i, engine) = fixture();
        let hits = engine.search("\"quick brown\"", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt"]);
        // The reversed phrase should not match.
        assert!(engine.search("\"brown quick\"", 10).unwrap().is_empty());
    }

    #[test]
    fn proximity() {
        let (_s, _i, engine) = fixture();
        // In file1 ("quick brown fox") the terms are one position apart from
        // an exact phrase, so a slop of >= 1 matches.
        let hits = engine.search("\"quick fox\"~2", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt"]);
        // The exact phrase (no slop) requires them adjacent, so it misses.
        assert!(engine.search("\"quick fox\"", 10).unwrap().is_empty());
    }

    #[test]
    fn snippet_highlights_match() {
        let (_s, _i, engine) = fixture();
        let hits = engine.search("fox", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].snippet.contains("<b>fox</b>"),
            "snippet should highlight the matched term, got: {}",
            hits[0].snippet
        );
    }

    #[test]
    fn wildcard_suffix() {
        let (_s, _i, engine) = fixture();
        // "qui*" -> quick (file1 and file3)
        let hits = engine.search("qui*", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt", "file3.txt"]);
    }

    #[test]
    fn wildcard_single_char() {
        let (_s, _i, engine) = fixture();
        // "?og" -> dog (file2 and file3)
        let hits = engine.search("?og", 10).unwrap();
        assert_eq!(names(&hits), vec!["file2.txt", "file3.txt"]);
    }

    #[test]
    fn regex() {
        let (_s, _i, engine) = fixture();
        // /jump.*/ -> "jumps" in file1
        let hits = engine.search("/jump.*/", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt"]);
    }

    #[test]
    fn fuzzy() {
        let (_s, _i, engine) = fixture();
        // "quik~1" is edit-distance 1 from "quick" (file1 and file3)
        let hits = engine.search("quik~1", 10).unwrap();
        assert_eq!(names(&hits), vec!["file1.txt", "file3.txt"]);
    }

    #[test]
    fn wildcard_combined_with_boolean() {
        let (_s, _i, engine) = fixture();
        // quick (via wildcard) AND dog -> only file3
        let hits = engine.search("qui* AND dog", 10).unwrap();
        assert_eq!(names(&hits), vec!["file3.txt"]);
    }

    #[test]
    fn negation_only_group() {
        let (_s, _i, engine) = fixture();
        // "-fox" -> every indexed doc except file1
        let hits = engine.search("-fox", 10).unwrap();
        assert_eq!(names(&hits), vec!["file2.txt", "file3.txt"]);
    }
}
