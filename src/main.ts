import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

interface IndexInfo {
  name: string;
  dir: string;
  folders: string[];
  doc_count: number;
  last_built: number | null;
}

interface SearchHit {
  path: string;
  filename: string;
  ext: string;
  size: number;
  modified: number;
  score: number;
  snippet: string;
}

interface BuildProgress {
  name: string;
  indexed: number;
  total: number;
}

// --- App state -------------------------------------------------------------

let indexes: IndexInfo[] = [];
let selected: string | null = null;
let newFolders: string[] = [];
const building = new Map<string, BuildProgress>();

// --- Element helpers -------------------------------------------------------

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const indexList = $<HTMLDivElement>("indexList");
const searchInput = $<HTMLInputElement>("searchInput");
const statusEl = $<HTMLDivElement>("status");
const resultsEl = $<HTMLDivElement>("results");
const modal = $<HTMLDivElement>("modal");
const indexNameInput = $<HTMLInputElement>("indexName");
const folderListEl = $<HTMLDivElement>("folderList");
const modalError = $<HTMLDivElement>("modalError");
const helpModal = $<HTMLDivElement>("helpModal");
const searchSpinner = $<HTMLSpanElement>("searchSpinner");
const toastContainer = $<HTMLDivElement>("toasts");

// --- Toasts ----------------------------------------------------------------

type ToastKind = "info" | "success" | "error";

function toast(message: string, kind: ToastKind = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  el.addEventListener("click", () => dismissToast(el));
  toastContainer.append(el);
  requestAnimationFrame(() => el.classList.add("show"));
  window.setTimeout(() => dismissToast(el), kind === "error" ? 5000 : 3000);
}

function dismissToast(el: HTMLElement) {
  if (!el.isConnected) return;
  el.classList.remove("show");
  window.setTimeout(() => el.remove(), 250);
}

// --- Indexes ---------------------------------------------------------------

async function refreshIndexes() {
  indexes = await invoke<IndexInfo[]>("list_indexes");
  if (selected && !indexes.some((i) => i.name === selected)) {
    selected = null;
  }
  if (!selected && indexes.length > 0) {
    selected = indexes[0].name;
  }
  renderIndexList();
  updateSearchAvailability();
}

function renderIndexList() {
  indexList.replaceChildren();
  if (indexes.length === 0) {
    const empty = document.createElement("div");
    empty.className = "index-empty";
    empty.textContent = "No indexes yet.";
    indexList.append(empty);
    return;
  }

  for (const info of indexes) {
    const item = document.createElement("div");
    item.className = "index-item" + (info.name === selected ? " selected" : "");

    const name = document.createElement("div");
    name.className = "index-name";
    name.textContent = info.name;
    item.append(name);

    const meta = document.createElement("div");
    meta.className = "index-meta";
    const progress = building.get(info.name);
    if (progress) {
      meta.textContent =
        progress.total > 0 ? `Building… ${progress.indexed}/${progress.total}` : "Building…";
    } else {
      meta.textContent =
        `${info.doc_count} doc${info.doc_count === 1 ? "" : "s"}` +
        (info.last_built ? ` · ${timeAgo(info.last_built)}` : " · not built");
    }
    item.append(meta);

    if (progress && progress.total > 0) {
      const bar = document.createElement("div");
      bar.className = "progress";
      const fill = document.createElement("div");
      fill.className = "progress-fill";
      fill.style.width = `${Math.round((progress.indexed / progress.total) * 100)}%`;
      bar.append(fill);
      item.append(bar);
    }

    const actions = document.createElement("div");
    actions.className = "index-actions";
    const rebuild = button("Rebuild", "small secondary", (e) => {
      e.stopPropagation();
      void buildIndex(info.name);
    });
    const del = button("Delete", "small secondary danger", (e) => {
      e.stopPropagation();
      void deleteIndex(info.name);
    });
    rebuild.disabled = !!progress;
    del.disabled = !!progress;
    actions.append(rebuild, del);
    item.append(actions);

    item.addEventListener("click", () => {
      selected = info.name;
      renderIndexList();
      updateSearchAvailability();
      void runSearch();
    });

    indexList.append(item);
  }
}

function updateSearchAvailability() {
  const ready = selected !== null;
  searchInput.disabled = !ready;
  if (!ready) {
    searchInput.value = "";
    resultsEl.replaceChildren();
    statusEl.textContent = indexes.length
      ? "Select an index to search."
      : "Create an index to get started.";
  } else {
    statusEl.textContent = `Searching “${selected}”.`;
  }
}

// --- Build / delete --------------------------------------------------------

async function buildIndex(name: string) {
  building.set(name, { name, indexed: 0, total: 0 });
  renderIndexList();
  toast(`Building “${name}”…`);
  try {
    const info = await invoke<IndexInfo>("build_index", { name });
    toast(`“${name}” built — ${info.doc_count} doc${info.doc_count === 1 ? "" : "s"}.`, "success");
  } catch (e) {
    toast(`Build failed: ${e}`, "error");
  } finally {
    building.delete(name);
    await refreshIndexes();
    if (selected === name) void runSearch();
  }
}

async function deleteIndex(name: string) {
  if (!confirm(`Delete index “${name}”? This removes its index data, not your files.`)) {
    return;
  }
  try {
    await invoke("delete_index", { name });
    toast(`Deleted “${name}”.`);
  } catch (e) {
    toast(`Delete failed: ${e}`, "error");
  }
  await refreshIndexes();
}

// --- Search ----------------------------------------------------------------

let searchSeq = 0;

async function runSearch() {
  if (!selected) return;
  const query = searchInput.value.trim();
  const seq = ++searchSeq;

  if (!query) {
    resultsEl.replaceChildren();
    statusEl.textContent = `Searching “${selected}”.`;
    searchSpinner.hidden = true;
    return;
  }

  searchSpinner.hidden = false;
  try {
    const hits = await invoke<SearchHit[]>("search", { index: selected, query, limit: 50 });
    if (seq !== searchSeq) return; // a newer search superseded this one
    renderResults(hits, query);
  } catch (e) {
    if (seq !== searchSeq) return;
    resultsEl.replaceChildren();
    statusEl.textContent = `Query error: ${e}`;
  } finally {
    if (seq === searchSeq) searchSpinner.hidden = true;
  }
}

function renderResults(hits: SearchHit[], query: string) {
  resultsEl.replaceChildren();
  statusEl.textContent = hits.length
    ? `${hits.length} result${hits.length === 1 ? "" : "s"} for “${query}”.`
    : `No results for “${query}”.`;

  for (const hit of hits) {
    const card = document.createElement("div");
    card.className = "result";

    const head = document.createElement("div");
    head.className = "result-head";
    const fname = document.createElement("button");
    fname.className = "result-name";
    fname.textContent = hit.filename || hit.path;
    fname.title = "Open file";
    fname.addEventListener("click", () => openFile(hit.path));
    head.append(fname);
    head.append(button("Reveal", "small secondary", () => revealFile(hit.path)));
    card.append(head);

    const path = document.createElement("div");
    path.className = "result-path";
    path.textContent = hit.path;
    card.append(path);

    if (hit.snippet) {
      const snippet = document.createElement("div");
      snippet.className = "result-snippet";
      snippet.innerHTML = hit.snippet; // safe: server escapes text and only adds <b>
      card.append(snippet);
    }

    const meta = document.createElement("div");
    meta.className = "result-meta";
    meta.textContent = `${formatSize(hit.size)} · ${formatDate(hit.modified)}`;
    card.append(meta);

    resultsEl.append(card);
  }
}

async function openFile(path: string) {
  try {
    await invoke("open_path", { path });
  } catch (e) {
    toast(`Couldn't open file: ${e}`, "error");
  }
}

async function revealFile(path: string) {
  try {
    await invoke("reveal_path", { path });
  } catch (e) {
    toast(`Couldn't reveal file: ${e}`, "error");
  }
}

// --- New-index modal -------------------------------------------------------

function openModal() {
  newFolders = [];
  indexNameInput.value = "";
  modalError.textContent = "";
  renderFolderList();
  modal.hidden = false;
  indexNameInput.focus();
}

function closeModal() {
  modal.hidden = true;
}

function renderFolderList() {
  folderListEl.replaceChildren();
  if (newFolders.length === 0) {
    const empty = document.createElement("div");
    empty.className = "folder-empty";
    empty.textContent = "No folders added.";
    folderListEl.append(empty);
    return;
  }
  newFolders.forEach((folder, i) => {
    const row = document.createElement("div");
    row.className = "folder-row";
    const span = document.createElement("span");
    span.textContent = folder;
    row.append(
      span,
      button("✕", "small secondary", () => {
        newFolders.splice(i, 1);
        renderFolderList();
      }),
    );
    folderListEl.append(row);
  });
}

async function addFolder() {
  const picked = await openDialog({ directory: true, multiple: true });
  if (!picked) return;
  const list = Array.isArray(picked) ? picked : [picked];
  for (const p of list) {
    if (!newFolders.includes(p)) newFolders.push(p);
  }
  renderFolderList();
}

async function confirmCreate() {
  const name = indexNameInput.value.trim();
  if (!name) {
    modalError.textContent = "Please enter a name.";
    return;
  }
  if (newFolders.length === 0) {
    modalError.textContent = "Add at least one folder.";
    return;
  }
  try {
    await invoke<IndexInfo>("create_index", { name, folders: newFolders });
  } catch (e) {
    modalError.textContent = `${e}`;
    return;
  }
  closeModal();
  selected = name;
  await refreshIndexes();
  void buildIndex(name);
}

// --- Formatting ------------------------------------------------------------

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[i]}`;
}

function formatDate(millis: number): string {
  if (!millis) return "unknown date";
  return new Date(millis).toLocaleString();
}

function timeAgo(millis: number): string {
  const secs = Math.floor((Date.now() - millis) / 1000);
  if (secs < 60) return "just now";
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

function button(
  label: string,
  className: string,
  onClick: (e: MouseEvent) => void,
): HTMLButtonElement {
  const b = document.createElement("button");
  b.className = className;
  b.textContent = label;
  b.addEventListener("click", onClick);
  return b;
}

function debounce<A extends unknown[]>(fn: (...args: A) => void, ms: number) {
  let timer: number | undefined;
  return (...args: A) => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => fn(...args), ms);
  };
}

// --- Wire up ---------------------------------------------------------------

const debouncedSearch = debounce(() => void runSearch(), 180);
searchInput.addEventListener("input", debouncedSearch);
searchInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") void runSearch();
});

$("newIndexBtn").addEventListener("click", openModal);
$("addFolderBtn").addEventListener("click", () => void addFolder());
$("confirmCreate").addEventListener("click", () => void confirmCreate());
$("cancelCreate").addEventListener("click", closeModal);
modal.addEventListener("click", (e) => {
  if (e.target === modal) closeModal();
});

$("helpBtn").addEventListener("click", () => {
  helpModal.hidden = false;
});
$("closeHelp").addEventListener("click", () => {
  helpModal.hidden = true;
});
helpModal.addEventListener("click", (e) => {
  if (e.target === helpModal) helpModal.hidden = true;
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    helpModal.hidden = true;
    closeModal();
  }
});

void listen<BuildProgress>("build-progress", (event) => {
  building.set(event.payload.name, event.payload);
  renderIndexList();
});

void refreshIndexes();
