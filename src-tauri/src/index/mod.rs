//! Index construction: schema definition and the folder-walking builder.

pub mod builder;
pub mod schema;

pub use builder::{build_index, BuildOptions, BuildProgress, BuildStats, DEFAULT_EXTENSIONS};
pub use schema::{build_schema, Fields};
