//! A tiny command-line harness for trying the index/search core before the
//! desktop UI is wired up (Milestone 6).
//!
//! Usage:
//!   cargo run --example cli -- build  <folder> [index_dir]
//!   cargo run --example cli -- search <query>  [index_dir]
//!
//! `index_dir` defaults to ./.cfsearch_index, so a `build` followed by a
//! `search` (from the same working directory) just works.

use std::env;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Result};
use cfsearch_lib::index::{build_index, BuildOptions};
use cfsearch_lib::search::SearchEngine;

const DEFAULT_INDEX_DIR: &str = ".cfsearch_index";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        print_usage();
        std::process::exit(1);
    }

    match args[1].as_str() {
        "build" => build(&args),
        "search" => search(&args),
        other => {
            eprintln!("unknown command: {other}\n");
            print_usage();
            std::process::exit(1);
        }
    }
}

fn build(args: &[String]) -> Result<()> {
    let folder = PathBuf::from(&args[2]);
    if !folder.is_dir() {
        bail!("not a folder: {}", folder.display());
    }
    let index_dir = index_dir_arg(args, 3);

    println!("Building index of {} ...", folder.display());
    let stats = build_index(
        &index_dir,
        &[folder],
        &BuildOptions::default(),
        |p| {
            if p.total > 0 && (p.indexed == p.total || p.indexed % 25 == 0) {
                print!("\r  indexing {}/{}", p.indexed, p.total);
                let _ = std::io::stdout().flush();
            }
        },
    )?;

    println!(
        "\rDone. Indexed {} file(s); {} of {} seen were skipped.\nIndex stored at {}",
        stats.indexed,
        stats.skipped,
        stats.files_seen,
        index_dir.display()
    );
    Ok(())
}

fn search(args: &[String]) -> Result<()> {
    let query = &args[2];
    let index_dir = index_dir_arg(args, 3);

    let engine = SearchEngine::open(&index_dir)?;
    let hits = engine.search(query, 20)?;

    if hits.is_empty() {
        println!("No results for {query:?}.");
        return Ok(());
    }

    println!("{} result(s) for {query:?}:\n", hits.len());
    for (i, h) in hits.iter().enumerate() {
        println!("{:>2}. {}  (score {:.2})", i + 1, h.path, h.score);
        let snippet = render_snippet(&h.snippet);
        if !snippet.is_empty() {
            println!("    {snippet}");
        }
        println!();
    }
    Ok(())
}

/// Resolve the optional positional index-dir argument, with a default.
fn index_dir_arg(args: &[String], idx: usize) -> PathBuf {
    args.get(idx)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_INDEX_DIR))
}

/// Turn the HTML snippet into something readable in a terminal: `<b>` becomes
/// ANSI bold and the basic HTML entities are unescaped.
fn render_snippet(snippet: &str) -> String {
    snippet
        .replace("<b>", "\u{1b}[1m")
        .replace("</b>", "\u{1b}[0m")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace(['\n', '\r'], " ")
}

fn print_usage() {
    eprintln!("cfSearch CLI (dev harness)\n");
    eprintln!("  cargo run --example cli -- build  <folder> [index_dir]");
    eprintln!("  cargo run --example cli -- search <query>  [index_dir]\n");
    eprintln!("Query syntax: term  \"exact phrase\"  a AND b  a OR b  a -b  \"a b\"~3");
}
