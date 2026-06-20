//! Tantivy schema for cfSearch indexes.
//!
//! One document per indexed file. `path` is the unique key used for
//! incremental upsert/delete; `content` is stored so we can generate
//! highlighted snippets at search time.

use tantivy::schema::*;

/// Handles to every field in the schema, resolved once at build time so
/// callers don't have to look fields up by name repeatedly.
#[derive(Clone, Copy)]
pub struct Fields {
    pub path: Field,
    pub filename: Field,
    pub ext: Field,
    pub content: Field,
    pub modified: Field,
    pub size: Field,
}

/// Build the cfSearch schema and return it alongside resolved field handles.
pub fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();

    // Absolute path: indexed verbatim (not tokenized) so it can serve as a
    // unique key for incremental delete/upsert. Stored for display.
    let path = builder.add_text_field("path", STRING | STORED);
    // Filename: tokenized so users can match on it, and stored for display.
    let filename = builder.add_text_field("filename", TEXT | STORED);
    // Extension: verbatim + fast so we can filter/facet by file type.
    let ext = builder.add_text_field("ext", STRING | STORED | FAST);
    // Full text: tokenized + stored (stored enables snippet highlighting).
    let content = builder.add_text_field("content", TEXT | STORED);
    // Modified time (unix seconds): fast for sorting + incremental compare.
    let modified = builder.add_i64_field("modified", STORED | FAST);
    // File size in bytes: fast for sorting, stored for display.
    let size = builder.add_u64_field("size", STORED | FAST);

    let schema = builder.build();
    (
        schema,
        Fields {
            path,
            filename,
            ext,
            content,
            modified,
            size,
        },
    )
}
