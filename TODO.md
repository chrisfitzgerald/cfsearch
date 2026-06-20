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
- [x] `mod.rs` — `SearchEngine` over `QueryParser` (content + filename): boolean (AND/OR/NOT, +/-), phrase, proximity slop `"a b"~N`; returns `SearchHit` (path, filename, ext, size, modified, score, snippet)
- [x] `snippet.rs` — highlighted excerpts via Tantivy `SnippetGenerator` over stored `content` (`<b>` highlights, escaped fallback to file start)
- [x] Unit tests (8): single term, AND/OR/NOT, phrase (+reversed miss), proximity slop boundary, snippet highlight
- [ ] (note) `query.rs` translation layer arrives in M4 for wildcard/regex/fuzzy

## Milestone 4 — Advanced query syntax (`search/query.rs`)
- [x] `QueryBuilder`: lexer + OR-of-AND-groups model (`-`/`NOT` negation, `+`, redundant `AND`)
- [x] Wildcards (`invoic*`, `organi?e`) → glob→regex → `RegexQuery` on `content`
- [x] Regex (`/pattern/`) → `RegexQuery` directly (case-insensitive: pattern lowercased)
- [x] Fuzzy (`term~`, `term~N`) → `FuzzyTermQuery`, distance clamped to 2
- [x] Combine sub-queries with `BooleanQuery`; plain terms/phrases delegated to `QueryParser`
- [x] Unit tests per operator (6 new; 16 total passing) + CLI demo on real files
- Known limitation: wildcard/regex/fuzzy hits use the file-start snippet (no term highlight)

## Milestone 5 — Incremental + multi-index (`index/store.rs`)
- [x] `IndexStore`: named indexes, each in its own slugified subfolder under a store root
- [x] `indexes.json` manifest: name, dir, source folders, doc_count, last_built (millis)
- [x] `update_index`: incremental — compare file mtime (millis) vs stored; only re-read changed
- [x] Delete docs whose files are gone / no longer eligible (`delete_term` on `path`)
- [x] Reader uses `ReloadPolicy::Manual` (no watcher thread); `delete` retries (Windows mmap release)
- [x] Tests (4 new; 20 total): incremental add/modify/remove, create→build→search→delete, dup-name, slugify

## Milestone 6 — IPC + UI wiring
- [x] `commands.rs` — `list_indexes`, `create_index`, `build_index` (progress events), `delete_index`, `search`, `open_path`, `reveal_path`
- [x] `state.rs` — `AppState` (store root under app-data + write lock); registered in `lib.rs` setup
- [x] Emit `build-progress` events (indexed / total); search opens a fresh engine per call (no held mmaps)
- [x] UI left panel — index list: New-index modal w/ folder picker (dialog plugin), Rebuild/Delete, live progress bar, doc count + last-built
- [x] UI top — search box (debounced + Enter) with syntax-hint helper
- [x] UI main — results: filename (click=open), path, highlighted snippet, size/date, Reveal button
- [x] Frontend type-checks + builds; app launches with new UI

## Milestone 7 — Polish
- [x] Minimal clean CSS (whitespace, monospace snippets, system light/dark)
- [x] Syntax-hint UI: compact hint bar + full "Syntax help" reference modal (Esc/backdrop to close)
- [x] Empty states (no indexes / select an index / no results); inline query + build/delete errors
- [ ] Further refinements as they come up

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
