<!-- From: /Users/qichengwen/My_APP_UI/Argus/AGENTS.md -->
# Argus — Agent Guide

This file is written for AI coding agents. It assumes you know nothing about the project. Read this before making non-trivial changes.

---

## Project overview

**Argus** is a local-first desktop research workspace for academic papers. It bundles PDF reading, note-taking, metadata extraction, arXiv tracking, paper relationship maps, library-wide RAG search, embedding-space visualization, and AI-assisted reading into one application.

- **Frontend:** Vue 3 + TypeScript + Vite + Pinia + vue-i18n.
- **Desktop shell:** Tauri v2 (Rust backend, WebKit-based WebView frontend).
- **Target platforms:** macOS (primary) and Windows. Linux is not currently released.
- **Data model:** Everything is stored locally in a user-chosen library folder. The app uses a hybrid of plain JSON/text files, SQLite FTS5 for full-text search, and SQLite vector tables for RAG.

> [!CAUTION]
> Most of this project was generated or heavily assisted by AI. The app is experimental and under active debugging. Keep backups of any real literature library.

---

## Repository layout

```
Argus/
├── src/                    # Vue/TypeScript frontend
│   ├── App.vue             # Root view selector (uses Tauri window label)
│   ├── main.ts             # Frontend entry point
│   ├── assets/             # Icons, provider/model logos, CSS design tokens
│   ├── components/         # Vue SFCs (feature folders: tabs/, canvas/, settings/)
│   ├── i18n/               # vue-i18n messages (zh + en)
│   ├── stores/             # Pinia stores + a few reactive helper modules
│   ├── types/              # Shared TypeScript types
│   ├── utils/              # Frontend utilities
│   └── views/              # Top-level window views
├── src-tauri/              # Rust backend
│   ├── src/                # Rust modules (commands, AI, RAG, OCR, etc.)
│   ├── capabilities/       # Tauri v2 capability declarations
│   ├── icons/              # App icons
│   ├── Cargo.toml          # Rust package manifest
│   └── tauri.conf.json     # Tauri app config
├── scripts/                # Node setup scripts
├── public/vditor/          # Copied Vditor editor assets (postinstall)
├── docs/images/            # Screenshots for README
├── .github/workflows/      # Release CI/CD
├── package.json            # Node manifest
├── vite.config.ts          # Vite config
├── tsconfig.json           # TypeScript config (app)
└── tsconfig.node.json      # TypeScript config (vite config)
```

---

## Technology stack

### Frontend

| Layer | Choice |
|-------|--------|
| Framework | Vue 3 (Composition API, `<script setup lang="ts">`) |
| Build tool | Vite 6 |
| State | Pinia 2 |
| i18n | vue-i18n 9 (locales: `zh` default, `en`) |
| PDF | pdfjs-dist v5 (legacy worker for older macOS) |
| Markdown / math | marked, katex, mermaid, highlight.js, dompurify |
| Editors | vditor (notes), @milkdown packages also present |
| Graph canvas | @vue-flow/core + background/controls/minimap |
| Virtual list | vue-virtual-scroller |
| RAG chunking | llamaindex (browser bundle) |

### Backend / desktop shell

| Layer | Choice |
|-------|--------|
| Shell | Tauri v2 |
| Language | Rust (edition 2021, minimum Rust 1.77.2) |
| Async runtime | Tokio (`full`) |
| HTTP client | reqwest |
| PDF parsing | lopdf, pdf-extract |
| OCR | macOS Vision framework first, fallback to tesseract / pdftoppm |
| Database | rusqlite (bundled) for FTS5 and vector store |
| Encryption | aes-gcm + rand for API key encryption |
| Plugins | dialog, store, window-state, http, updater, process |

---

## Build and run commands

All commands are run from the repository root.

### Prerequisites

- Node.js 22+ and npm.
- Rust stable toolchain.
- On macOS: Xcode / command-line tools for building the Tauri app.
- On Windows (CI only): ImageMagick `magick` for generating `icon.ico` if missing.

### Development

```bash
# Install dependencies and copy Vditor assets to public/vditor/
npm install

# Run the Vite dev server only (frontend in browser/WebView)
npm run dev

# Run the full Tauri desktop app in dev mode
npm run tauri dev
```

Vite dev server runs on `http://localhost:1420` (HMR on `1421` when `TAURI_DEV_HOST` is set).

### Production build

```bash
# Type-check and bundle the frontend to dist/
npm run build

# Fast frontend build without type checking
npm run build:fast

# Build the Tauri desktop app installer for the current platform
npm run tauri build
```

### Other useful commands

```bash
npm run preview      # Preview the built dist/ bundle
npm run tauri        # Proxy to the Tauri CLI
cargo test -p argus  # Run the few Rust unit tests
```

---

## Architecture

### Window-based view routing

The app uses multiple Tauri windows rather than browser-style routing. `src/App.vue` selects the top-level view by calling `getCurrentWebviewWindow().label`:

| Window label | View rendered | Purpose |
|--------------|---------------|---------|
| `main` | `MainView` | Primary 3-column workspace |
| `arxiv` | `ArxivView` | arXiv / bioRxiv recommendation inbox |
| `canvas` | `CanvasView` | Paper relationship canvas (Vue Flow) |
| `library-chat` | `LibraryChatView` | Library-wide RAG chat |
| `paper-ai` | `PaperAiView` | Per-paper AI chat |
| `embedding-map` | `EmbeddingMapView` | 2-D visualization of the vector embedding space |
| `note-window-*` | `NoteWindowView` | Standalone note editor |

All top-level views are loaded with `defineAsyncComponent` so each window only loads the code it needs.

### Frontend state management (Pinia stores)

Stores live in `src/stores/` and use the Composition API style (`defineStore('id', () => {...})`).

| Store | Responsibility |
|-------|----------------|
| `library.ts` | Current library path, paper index, tag list, scan/refresh |
| `reader.ts` | Open PDF tabs, active tab, reading state, highlights |
| `selection.ts` | Selected paper, sidebar nav state, search results |
| `collections.ts` | Hierarchical collections and paper assignments |
| `import.ts` | PDF / URL import job queue and orchestration |
| `paperTasks.ts` | In-progress AI tasks per paper and progress events |
| `ai.ts` | AI provider/model settings |
| `settings.ts` | App settings (theme, prompts, extraction defaults) |
| `rag.ts` | RAG provider, embedding model, vector store status |
| `arxiv.ts` | arXiv inbox, config, schedule status, analysis |
| `canvas.ts` | Canvas list, current canvas, auto-save |

`snippetLibrary.ts`, `translationHistory.ts`, and `update.ts` are reactive helper modules, not Pinia stores.

### Backend modules (Rust)

| Module | Responsibility |
|--------|----------------|
| `commands.rs` | All `#[tauri::command]` handlers exposed to the frontend |
| `models.rs` | Core data structures (`PaperMeta`, `Highlight`, `Note`, `Collection`, `AiProvider`, etc.) |
| `library.rs` | Library initialization and incremental scan |
| `paper.rs` | Per-paper directory/file I/O with path validation and atomic writes |
| `metadata.rs` | PDF text extraction and external metadata fetching (arXiv, Crossref, Semantic Scholar) |
| `extraction.rs` | Full-text extraction pipeline with OCR fallback |
| `ocr.rs` | OCR via macOS Vision, tesseract, pdftoppm |
| `collections.rs` | Collection CRUD and nested moves |
| `search.rs` | SQLite FTS5 full-text index |
| `rag.rs` | Vector store, embedding storage, cosine similarity search |
| `ai_manager.rs` | AI provider CRUD and AES-256-GCM API key encryption |
| `llm.rs` | OpenAI-compatible / Anthropic chat, embeddings, OpenRouter, token usage |
| `ai_summary.rs` | Generate AI paper summaries and abstract extraction |
| `copilot.rs` | Per-paper and library-wide chat, chat history persistence |
| `arxiv.rs` / `arxiv_scheduler.rs` | arXiv/bioRxiv fetching, inbox storage, scheduled catch-up |
| `canvas.rs` / `canvas_enhance.rs` | Canvas CRUD, edge suggestions, auto-layout, export |
| `snippets.rs` | Snippet library CRUD |
| `token_usage.rs` | Token and USD cost tracking |
| `url_import.rs` | Import from ACL Anthology, OpenReview, arXiv, direct PDF |
| `settings.rs` | `config.json` settings I/O |
| `path_guard.rs` | Path-segment validation against traversal attacks |
| `security_bookmark.rs` | macOS security-scoped bookmark persistence |
| `fsutil.rs` | Shared filesystem helpers |

### Data persistence

The library root contains:

```
<library>/
├── .argus/
│   ├── config.json          # Library config, app settings, RAG/arXiv/canvas settings
│   ├── index.json           # Rebuildable paper index cache
│   ├── search.db            # SQLite FTS5 full-text index
│   ├── search.version       # Index version marker
│   ├── vectors.sqlite       # RAG vector store
│   ├── vectors_meta.json    # Vector store metadata
│   ├── ai_providers.json    # AI provider configs
│   ├── api_keys.json        # Encrypted API keys
│   ├── token_usage.jsonl    # Token usage log
│   ├── library_chat.json    # Legacy single-thread library chat history (unused by UI)
│   ├── library_chats.json   # Library "智能问答" conversations (multi-conversation)
│   └── collections.json     # Collection tree and assignments
├── papers/<slug>/           # One folder per paper
│   ├── meta.json
│   ├── paper.pdf
│   ├── notes/               # Multi-note storage
│   ├── highlights.json
│   ├── fulltext.txt
│   ├── reading_state.json
│   ├── .status.json
│   ├── chat.json
│   └── ai_conversations.json
├── canvases/                # Canvas JSON files
├── inbox/                   # arXiv/bioRxiv daily inbox JSON
└── snippets/                # Snippet library JSON
```

Global app state (last library path, window sizes, security bookmarks) is stored via `tauri-plugin-store` in `settings.json` inside the app data directory.

Key design points:

- `index.json`, `search.db`, and `vectors.sqlite` are rebuildable caches; the JSON/text files in each paper folder are the source of truth.
- Rust writes files atomically (write `.tmp`, then `rename`) where possible.
- API keys are encrypted with a per-library random master key stored in `.argus/.keymaster`.

---

## Frontend ↔ backend communication

- **Commands:** Frontend calls Rust with `invoke` from `@tauri-apps/api/core`. Commands are registered in `src-tauri/src/lib.rs` via `tauri::generate_handler!`.
- **Events:** Rust pushes progress/cancellation events with `app.emit()`; frontend listens with `listen` from `@tauri-apps/api/event`. Examples: `ai-summary-progress`, `arxiv-fetch-due`, `arxiv-analysis`, `extraction_progress`, `extraction_done`, `library-updated`.
- **Cross-window events:** Some decoupled UI updates use browser `CustomEvent` on `window` (e.g., `argus-paper-meta-updated`, `argus-switch-sidebar-tab`).

The command surface is large (~100+ commands). See `src-tauri/src/commands.rs` for the authoritative list, grouped into library management, single-paper I/O, collections, metadata/import, AI providers, chat/copilot, RAG, arXiv, canvas, embedding map, snippets, and window/system operations.

---

## Code style guidelines

### General

- No ESLint, Prettier, or editor config is currently set up. The only enforced code-quality step is `vue-tsc --noEmit` during `npm run build`.
- Follow the existing style in each file. Frontend uses Vue Composition API with `<script setup lang="ts">`. Rust uses idiomatic 2021 edition style.

### File naming

- Vue SFCs: PascalCase (`PdfViewer.vue`, `SettingsModal.vue`).
- Rust modules: `snake_case.rs` (`ai_summary.rs`, `arxiv_scheduler.rs`).
- Frontend subfolders group by feature:
  - `src/components/tabs/`
  - `src/components/canvas/`
  - `src/components/settings/`
  - `src/views/`, `src/stores/`, `src/types/`, `src/utils/`

### Styling

- Use the CSS design tokens in `src/assets/main.css` instead of hard-coded colors.
- Themes are applied via `data-theme` (`system`, `light`, `dark`, `warm`, `forest`, `rose`). When no `data-theme` is set, the dark palette follows `prefers-color-scheme: dark`.
- Common tokens: `--bg-primary`, `--bg-secondary`, `--text-primary`, `--text-secondary`, `--accent`, `--accent-hover`, `--border-subtle`, `--divider`, `--shadow-sm/md/lg`, `--radius-sm/md/lg`.
- The design is intentionally flat: no gradients or inner shadows on accent elements.

### TypeScript

- Strict mode is enabled.
- `@/*` maps to `./src/*`.
- `noEmit` is enabled; Vite handles transpilation.

### Rust

- Keep blocking I/O and CPU-heavy work off the Tauri async runtime by using `spawn_blocking` (already used for PDF extraction, metadata, search indexing, and vector writes).
- Validate any user-provided path segment with the helpers in `path_guard.rs`.
- Do not store plaintext API keys; use `ai_manager.rs` encryption helpers.

---

## Testing instructions

Testing is minimal.

- **Frontend:** No test runner or test files.
- **Backend:** A small number of Rust unit tests exist in `src-tauri/src/path_guard.rs` and `src-tauri/src/collections.rs`.
- **CI:** The release workflow does not run tests.

To run the existing Rust tests locally:

```bash
cargo test -p argus
```

When adding significant backend logic, prefer adding `#[cfg(test)]` modules in the relevant Rust file.

---

## Security considerations

- **Path traversal:** `path_guard.rs` validates slugs, note IDs, canvas IDs, and library IDs. Do not bypass it when constructing filesystem paths.
- **macOS sandbox:** `security_bookmark.rs` creates and restores security-scoped bookmarks for the library root. Access must be started/stopped with bookmark APIs.
- **API keys:** Encrypted with AES-256-GCM using a per-library random master key. The master key lives in `.argus/.keymaster`.
- **CSP:** `tauri.conf.json` sets `"csp": null`. Be cautious when rendering untrusted HTML/markdown; the frontend already uses DOMPurify.
- **HTTP permissions:** `src-tauri/capabilities/default.json` only allows `https://export.arxiv.org/**` and `https://api.biorxiv.org/**` for built-in fetch. Other HTTP calls go through `tauri-plugin-http` and must be declared in capabilities.
- **URL opening:** `open_url` only permits `http://` and `https://` schemes.

---

## Deployment and release process

The release pipeline is defined in `.github/workflows/release.yml`.

1. **Trigger:** Push a tag `v*` or run the workflow manually with a version string.
2. **Create release:** A draft GitHub Release is created from the tag message.
3. **Build:** Tauri builds run on `macos-latest` and `windows-latest`.
   - The workflow patches `tauri.conf.json` with the release version and enables bundling / updater artifacts.
   - Windows removes `node_modules`/`package-lock.json` and regenerates `icon.ico` if needed.
   - `npm install` runs, which triggers the Vditor postinstall script.
   - `tauri-apps/tauri-action@v0` builds and uploads installers.
   - Required secrets: `GITHUB_TOKEN`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. **Publish:** The draft release is marked as published.

Updater endpoint configured in `tauri.conf.json`:

```
https://github.com/chenwen245299/Argus/releases/latest/download/latest.json
```

### macOS install note

Users downloading the `.dmg` must clear the quarantine flag before the app will open:

```bash
xattr -cr /Applications/Argus.app
```

---

## Postinstall setup

`scripts/setup-vditor.js` runs automatically after `npm install`. It copies `node_modules/vditor/dist` to `public/vditor/dist` so the Vditor editor assets are bundled into both dev and production builds. If Vditor notes do not render correctly, verify that `public/vditor/dist/` exists and matches the installed `vditor` version.

---

## Common pitfalls

- **Wrong window label:** Many stores initialize only for specific windows. Check `App.vue` before adding window-specific logic.
- **Path construction:** Always validate segments with `path_guard.rs` helpers; never concatenate raw user strings into filesystem paths.
- **Async blocking in Rust:** PDF extraction, metadata fetching, and vector DB writes are blocking. Mirror the existing `spawn_blocking` pattern.
- **i18n:** Default locale is `zh`. Add new keys to both `src/i18n/locales.ts` objects.
- **Rebuildable caches:** It is safe to delete `.argus/index.json`, `search.db`, and `vectors.sqlite`; the app can rebuild them from the paper folders.
- **Workers directory:** `src/workers/` is currently empty. Do not assume web workers exist.

---

## Useful entry points for changes

| Task | Start here |
|------|------------|
| Add a Tauri command | `src-tauri/src/commands.rs` + register in `src-tauri/src/lib.rs` |
| Add a frontend store | `src/stores/` following Composition API style |
| Add a settings section | `src/components/SettingsModal.vue` + `src/components/settings/` |
| Add a sidebar tab | `src/components/RightSidebar.vue` + `src/components/tabs/` |
| Change PDF rendering | `src/components/PdfViewer.vue` |
| Change RAG behavior | `src-tauri/src/rag.rs`, `src/stores/rag.ts`, `src/components/LibraryChat.vue` |
| Change AI chat | `src-tauri/src/copilot.rs`, `src-tauri/src/llm.rs`, `src/components/tabs/AiTab.vue` |
| Change canvas | `src/views/CanvasView.vue`, `src/components/canvas/`, `src-tauri/src/canvas*.rs` |
| Change import pipeline | `src/stores/import.ts`, `src-tauri/src/metadata.rs`, `src-tauri/src/url_import.rs` |
| Change themes | `src/assets/main.css`, `src/stores/settings.ts` |
| Change arXiv inbox | `src/views/ArxivView.vue`, `src/stores/arxiv.ts`, `src-tauri/src/arxiv*.rs` |
| Change embedding map | `src/views/EmbeddingMapView.vue`, `src-tauri/src/rag.rs` |

---

*Last updated: 2026-06-24. Keep this file in sync with major architectural changes.*
