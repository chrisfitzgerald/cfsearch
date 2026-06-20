# cfSearch — TODO

A minimal dtSearch-style text indexer & search desktop app.
**Stack:** Rust + Tantivy (search core) · Tauri 2 (desktop shell) · Vite + TypeScript (UI).
**Scope v1:** plain-text formats only · single-user local app.

---

## Milestone 1 — Scaffold
- [x] Install prerequisites: Rust 1.96 (rustup), MSVC C++ Build Tools, WebView2 (already present)
- [x] Run `create-tauri-app` with the vanilla-ts template into `cfSearch/` (Tauri 2, identifier `com.cfsearch.app`)
- [x] `npm install` — frontend builds (`npm run build` OK)
- [x] `cargo build` — Rust/Tauri backend compiles + links to `cfsearch.exe` (MSVC linker verified)
- [x] Confirm `npm run tauri dev` opens a window (default greet UI launched, build finished in 7.5s)
- [x] Commit baseline project structure (`git init`, baseline commit on `main`)

## Milestone 2 — Index core (`src-tauri/src/index/`)
- [x] `schema.rs` — Tantivy schema: `path` (STRING|STORED, unique key), `filename` (TEXT|STORED), `ext` (STRING|STORED|FAST), `content` (TEXT|STORED), `modified` (i64 FAST|STORED), `size` (u64 FAST|STORED)
- [x] `builder.rs` — walk folders with the `ignore` crate
- [x] Filter by text-extension allow-list (~70 text/source extensions; see `DEFAULT_EXTENSIONS`)
- [x] Skip oversized files (20 MB cap, configurable) and binary files (NUL-byte heuristic)
- [x] Encoding detection via `chardetng` 1.0 + decode via `encoding_rs`
- [x] Parallel read/decode with `rayon`, then sequential add to a single `IndexWriter` (full rebuild)
- [x] Unit tests (3, all passing): term search, size cap, idempotent rebuild
- [ ] (deferred to M5) Incremental rebuild + delete-missing

## Milestone 3 — Search core (`src-tauri/src/search/`)
- [ ] `query.rs` — Tantivy `QueryParser` path: boolean (AND/OR/NOT, +/-), phrase, proximity slop `"a b"~3`
- [ ] `snippet.rs` — highlighted excerpts via Tantivy `SnippetGenerator` over stored `content`
- [ ] Unit tests for boolean, phrase, proximity + snippet output

## Milestone 4 — Advanced query syntax (`search/query.rs`)
- [ ] Wildcards (`invoic*`, `organi?e`) → glob→regex → `RegexQuery` on `content`
- [ ] Regex (`/pattern/`) → `RegexQuery` directly
- [ ] Fuzzy (`term~`, `term~1`) → `FuzzyTermQuery` with edit distance
- [ ] Combine per-token sub-queries with a `BooleanQuery`
- [ ] Unit test per operator against fixtures

## Milestone 5 — Incremental + multi-index (`index/store.rs`)
- [ ] Named indexes, each in its own folder under the app data dir (`tauri::path`)
- [ ] `indexes.json` manifest: name, source folders, doc count, last-built time
- [ ] Incremental rebuild: compare file `modified` vs stored doc; only re-index changed
- [ ] Delete docs whose files no longer exist (`delete_term` on `path`)

## Milestone 6 — IPC + UI wiring
- [ ] `commands.rs` — `list_indexes`, `create_index`, `build_index` (async + progress events), `delete_index`, `search`, `open_path`, `reveal_in_explorer`
- [ ] Emit Tauri progress events (files scanned / indexed / total)
- [ ] UI left panel — index manager: create, pick folders (Tauri dialog), Build/Rebuild, progress bar, doc count + last-built
- [ ] UI top — search box with syntax-hint helper
- [ ] UI main — results list: filename, path, highlighted snippet, size/date; click to open; reveal in Explorer

## Milestone 7 — Polish
- [ ] Minimal clean CSS (whitespace, monospace snippets, system light/dark)
- [ ] Syntax-hint UI with examples of each operator
- [ ] Error states + empty states

## Milestone 8 — Package
- [ ] `cargo tauri build` → installer / native binary

---

## Verification
- [ ] `cargo test` covers index core + each operator: boolean, phrase, wildcard `inv*`, regex `/inv.*/`, proximity `"a b"~3`, fuzzy `term~1`
- [ ] `cargo tauri dev` end-to-end: create index from a real folder → progress bar completes → one search per operator returns correct hits + highlighted snippets → click result opens file + reveal in Explorer
- [ ] Edit/add/delete a source file, rebuild, confirm incremental update (changed doc updated, deleted file removed)
- [ ] `cargo tauri build` produces a launchable desktop app

## Out of scope for v1 (future)
- [ ] PDF/Office extraction (Apache Tika sidecar)
- [ ] Live file-watching (`notify` crate)
- [ ] Search history / saved searches
- [ ] Result sorting / faceting by ext/date
- [ ] Multi-user / network mode

## Key crates
`tauri` (2.x) · `tantivy` · `ignore` · `chardetng` + `encoding_rs` · `rayon` · `serde`/`serde_json` · `anyhow`/`thiserror` · `time`/`chrono`
