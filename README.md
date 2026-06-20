# cfSearch

A fast, minimal **local full‑text search** app for your documents — a lightweight, dtSearch‑style tool. Point it at folders of files on your PC, build an index, and search them instantly with a rich query syntax (boolean, phrases, wildcards, regex, fuzzy, proximity).

Built with **Rust + [Tantivy](https://github.com/quickwit-oss/tantivy)** (a Lucene‑class search engine) wrapped in a **[Tauri 2](https://tauri.app)** desktop app, with a clean vanilla‑TypeScript UI. It compiles to a single small native binary — no server, no browser, no cloud.

---

## Features

- 🔎 **Instant full‑text search** over folders of files, ranked by relevance (BM25).
- 🗂️ **Multiple named indexes**, each over one or more source folders.
- ⚡ **Incremental rebuilds** — only changed files are re‑read; deleted files are dropped.
- ✨ **Highlighted snippets** showing matches in context.
- 🧩 **Rich query syntax**: terms, exact phrases, `AND`/`OR`/`NOT`, exclusion, wildcards, regex, fuzzy, and proximity.
- 🖥️ **Single‑user desktop app** — your files never leave your machine.
- 🌓 Minimal UI with automatic light/dark.

**Scope (v1):** plain‑text formats only — `.txt`, `.md`, `.csv`, `.log`, `.json`, and ~70 source‑code/markup extensions. Binary files and files over 20 MB are skipped. (PDF/Office extraction is a possible future addition.)

---

## Install

Download an installer from a release build and run it:

- **`cfSearch_<version>_x64-setup.exe`** — Windows installer (NSIS). Recommended.
- **`cfSearch_<version>_x64_en-US.msi`** — Windows MSI (for managed deployment).

> The installer is currently unsigned, so Windows SmartScreen may warn on first run.

To produce these yourself, see **Build from source** below; they land in
`src-tauri/target/release/bundle/`.

---

## Usage

1. Click **➕ New index**, give it a name, and **Add folder…** (one or more).
2. **Create & build** — watch the progress bar in the sidebar.
3. Type in the search box. Results update as you type; press **Enter** to search immediately.
4. Click a result's **filename** to open the file, or **Reveal** to show it in Explorer.
5. **Rebuild** an index after changing its files (only changes are re‑indexed); **Delete** removes the index data (never your files).

Click **Syntax help** by the search box for an in‑app reference.

### Query syntax

| Example | Meaning |
|---|---|
| `budget` | a single word |
| `budget report` | both words (space = AND) |
| `"purchase order"` | an exact phrase |
| `invoice AND 2025` | both terms required |
| `cat OR dog` | either term |
| `report -draft` / `report NOT draft` | has *report*, excludes *draft* |
| `a b OR c` | grouped as `(a AND b) OR c` |
| `invoic*` | wildcard — `*` = any run of characters |
| `organi?e` | wildcard — `?` = exactly one character |
| `/colou?r/` | regex between slashes (matches whole words, case‑insensitive) |
| `paymnet~1` | fuzzy — allow 1 typo (max edit distance 2) |
| `"quick fox"~5` | proximity — the words within 5 of each other |

**Regex note:** patterns match whole indexed *words*, anchored end to end — so `/inv/` matches only the exact word "inv"; use `/inv.*/` for "starts with inv". Patterns can't span spaces; use a `"phrase"` for multi‑word matching.

---

## Where data is stored

- **App indexes:** `%APPDATA%\com.cfsearch.app\indexes\` (Windows). Each index is a subfolder of Tantivy data, plus an `indexes.json` manifest. This holds the extracted text and search structures — **not** copies of your files. Deleting an index never touches your documents.

---

## Build from source

### Prerequisites

- **Rust** (stable) — <https://rustup.rs>
- **Node.js** 18+ and npm
- **Windows:** the **MSVC C++ Build Tools** (for the linker) and the WebView2 runtime (preinstalled on Windows 10/11).
  Other platforms: see the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

### Run in development

```bash
npm install
npm run tauri dev
```

### Build installers

```bash
npm run tauri build
# → src-tauri/target/release/bundle/{nsis,msi}/...
```

### Run the tests

```bash
cd src-tauri
cargo test
```

### CLI harness (no UI)

A small command‑line tool exercises the same index/search engine — handy for scripting or quick checks:

```bash
cd src-tauri
cargo run --example cli -- build  "C:\path\to\folder"   # build an index
cargo run --example cli -- search "your query"          # search it
```

It stores its index in `.cfsearch_index` in the current directory (separate from the app's indexes).

---

## Project structure

```
cfSearch/
├─ src/                       # Frontend (Vite + TypeScript)
│  ├─ main.ts                 # UI logic + IPC calls
│  └─ styles.css
├─ index.html
└─ src-tauri/                 # Rust backend (Tauri app)
   ├─ src/
   │  ├─ index/               # Tantivy schema, folder builder, named-index store
   │  ├─ search/              # query parsing (incl. wildcard/regex/fuzzy) + snippets
   │  ├─ commands.rs          # Tauri IPC commands
   │  ├─ state.rs             # shared app state
   │  └─ lib.rs               # app setup + command registration
   └─ examples/cli.rs         # CLI dev harness
```

---

## Tech stack

| Layer | Choice |
|---|---|
| Search engine | [Tantivy](https://github.com/quickwit-oss/tantivy) |
| App shell | [Tauri 2](https://tauri.app) |
| Backend | Rust |
| Frontend | Vite + vanilla TypeScript |
| Indexing helpers | `ignore` (walk), `chardetng` + `encoding_rs` (decode), `rayon` (parallel) |
