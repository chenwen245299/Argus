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
│   ├── assets/             # Icons, provider/model logos, CSS design tokens (main.css) + theme palettes (themes.css)
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
| `mcp/` | Read-only MCP server for external agents, run as a stdio subprocess — see below |
| `offer_sync.rs` | Background re-read of model prices on launch, so a withdrawn free tier stops advertising itself |
| `path_guard.rs` | Path-segment validation against traversal attacks |
| `security_bookmark.rs` | macOS security-scoped bookmark persistence |
| `fsutil.rs` | Shared filesystem helpers |

### MCP server (`src-tauri/src/mcp/`)

An optional, **off by default** read-only MCP server exposing the library to
external agents (Claude Code, Claude Desktop, Codex).

| File | Responsibility |
|------|----------------|
| `mcp/mod.rs` | The on/off setting, library resolution, client config snippets, the stdio entry point |
| `mcp/server.rs` | `rmcp` tool declarations (names, JSON schemas, descriptions) |
| `mcp/tools.rs` | The read implementations — **and the security boundary** |
| `mcp/agent.rs` | The same tools in-process, for the app's own agent mode |
| `mcp/client.rs` | The *other* direction: Argus as an MCP client of other servers |

**Transport is stdio.** The client launches `Argus --mcp-stdio` as a subprocess
(`main.rs` checks the flag before any Tauri setup) and speaks newline-delimited
JSON-RPC over its stdin/stdout. There is no network listener, no port, and no
token; the process boundary is the whole transport. The same config works in
every client and needs no Node.js.

It reads the library folder directly, so it works whether or not the app is
running. Reads are safe alongside a live Argus: JSON is written atomically
(`fsutil::atomic_write_str`) and the SQLite caches use WAL. The only control is
`mcp_enabled` in the app-data store — the GUI writes it, the stdio process reads
it from disk and refuses to start when false.

Note that Claude Desktop's "Add custom connector" dialog cannot be used: it is
for *remote* servers reached from Anthropic's infrastructure. Local servers go in
`claude_desktop_config.json`.

**Read-only, by construction.** No tool accepts a filesystem path; callers pass a
slug or id, and path building goes through `paper::paper_dir` + `path_guard`.
There is therefore no reachable path from an MCP request to `api_keys.json`,
`.keymaster`, `ai_providers.json`, `config.json` or `token_usage.jsonl`. See the
table in `mcp/tools.rs`.

**Conversations are exposed, but redacted.** `library_chats.json` and the
per-paper `ai_conversations.json` are readable; `redact_answer` drops provider
identity, per-call cost and token counts, `contextContent`, `reasoningContent`,
and attachment `dataUrl` blobs (names survive). The legacy per-paper `chat.json`
is skipped — it mirrors the active conversation, so exposing it would duplicate
`ai_conversations.json`.

`get_library_stats` is the intended entry point for an agent meeting a library
for the first time: one incremental index scan yields counts by reading status,
year, file type and tag, plus the pipeline flags already carried in
`PaperIndexEntry::status`, so it costs about the same as one `list_papers` call
and replaces a series of filtered probes. `list_collections` reports both the
direct `paper_count` and the deduplicated `total_paper_count` across
descendants, along with a readable `path`.

When adding a tool: implement the read in `tools.rs` (never touch the filesystem
in `server.rs`), mark it `read_only_hint = true`, and add its name to
`EXPECTED_TOOLS` in `mcp/server.rs` — the test there fails otherwise, which is
the intended prompt to re-check the security model.

Two schema rules are enforced by tests, both learned the hard way — violating
either makes **every** tool on the server disappear from clients with no error
shown to the user:

- **`outputSchema` must describe an object.** A tool returning a bare `Vec<T>`
  emits `"type": "array"`, which clients reject — and they reject the whole
  `tools/list` response over it, not just the one tool. Wrap lists in
  `tools::ItemList<T>`.
- **No `$ref` / `$defs` may escape.** `schemars` factors nested structs into
  `$defs`; `flattened_tool_router` inlines them so schemas are self-contained,
  since client support for resolving references varies.

### Agent mode (library Q&A)

`knowledge_source: "agent"` on the `chat_with_library` command routes to
`copilot::chat_with_library_agent`, which hands the model the same tool surface
the MCP server exposes and lets it drive its own retrieval instead of receiving
a pre-built RAG context.

- `mcp::agent::tools()` / `mcp::agent::call()` expose the tools in-process. The
  dispatch in `mcp/agent.rs` is written by hand because invoking a `ToolRoute`
  needs a `RequestContext<RoleServer>` that only exists inside a live service;
  two tests keep it in sync with the declarations in both directions.
- `llm::stream_with_tools` is OpenAI-compatible only (DeepSeek, OpenRouter,
  Kimi, custom endpoints). `llm::supports_tool_calling` gates it so
  Anthropic/Ollama users get a clear message rather than a 400. It streams
  content deltas live *and* accumulates the `delta.tool_calls[i]` fragments,
  whose `function.arguments` arrive split across chunks and must be concatenated
  by `index` before they parse.
- `chat_with_library_agent` connects the external servers, then runs
  `run_agent_loop`; the split exists so the child processes are torn down on
  every exit path, cancellation included.
- The system prompt is user-editable (设置 → 智能问答 → Agent, key
  `agent_system_prompt`); blank falls back to `DEFAULT_AGENT_SYSTEM_PROMPT`.
  That default's substantive instruction is **collection-first retrieval**: walk
  `list_collections` → `list_papers(collection_id)` → narrow, and treat a
  whole-library keyword `list_papers` as the last resort. Left to itself a model
  reaches for the keyword sweep, which matches titles and ignores the structure
  the user built by hand. It also tells the model that `list_papers` /
  `search_papers` take `abstract_detail` (`preview` default / `full` / `none`),
  so how much abstract text comes back is the model's call, not a hardcoded cut.
  `agent_system_prompt` is shared with the keepalive —
  the two must send byte-identical system messages or the warmed prefix is not
  the one the next question sends.
- The loop is bounded by `MAX_AGENT_ROUNDS` (500); on hitting it the model gets
  one final tool-less turn to answer with what it has. Tool results are capped at
  `MAX_TOOL_RESULT_CHARS` and the truncation is told to the model so it pages.
- A failing tool is fed back as an error string rather than aborting, so the
  model can correct a bad slug or section name on the next round.
- **Usage is summed, not emitted per round.** `stream_with_tools` returns a
  `TurnUsage` instead of emitting one; the loop folds them and calls
  `llm::emit_usage` once at the end. Emitting per round both showed a cost strip
  during the first tool call and reported only that round's figures.
- Progress is emitted on `{event_name}-agent`, phases `servers` / `thinking` /
  `tool` / `result` / `answering` / `limit`.

**Prompt-cache keepalive (`cache_keepalive.rs`).** An agent turn sends a large
prefix (system prompt + 17 tool schemas + the conversation), and providers with
automatic prefix caching bill a repeat of it at roughly a tenth of the normal
rate — but only while the entry is warm; DeepSeek's expires after ~10 minutes
idle. After each agent answer, `chat_with_library_agent` arms a loop that
re-sends the same prefix every 5 minutes with `max_tokens: 1`.

- The warmed prefix is *not* the loop's internal transcript. It is what the next
  question will send: system + the clean user/assistant history + the answer
  just given. The loop's `tool` messages never reappear in a later request, so
  warming them would refresh a prefix nothing asks for. The `tools` array is
  snapshotted verbatim from the turn (external servers included) for the same
  reason — `agent_tool_defs` is shared so the two cannot drift.
- Gated by `is_worthwhile`: DeepSeek always (documented caching, and turn 1 has
  no hit to observe yet), everyone else only once a turn has actually reported
  `cache_hit_tokens > 0`. Against a provider with no cache the ping would be a
  full-price re-read every 5 minutes to save nothing.
- Stops on any of: the `library-chat` window being gone (checked before every
  ping, so a window torn down without front-end cleanup still ends it), an hour
  since the last question, or two consecutive failures.
- Recorded in the usage ledger under source `cache-keepalive`, so this
  background spend is visible rather than folded into the user's own turns.
- User-switchable in 设置 → 智能问答 → Agent (`agent_keep_cache_warm`, default on).
- Status reaches the chat window on the `cache-keepalive` event (`{active, model,
  pings, stopsAtMs, intervalSeconds}` / `{active: false, reason}`), which drives
  the breathing dot on the 知识来源 pill (and its counterpart in the conversation
  list). The status carries a `conversationId` the backend treats as opaque, so
  the indicator lands on the one conversation whose prefix is actually held. `disarm` is silent — `arm` calls it to
  replace its predecessor, and announcing there would blink the badge between
  every question; explicit stops go through `disarm_and_announce`.

**External MCP servers (`mcp/client.rs`).** Users can point agent mode at other
MCP servers, which Argus launches as stdio subprocesses exactly the way Claude
Desktop launches Argus. Configuration lives in the app-data store
(`mcp_external_servers`, `agent_max_rounds`) and is edited in 设置 → 智能问答.

- Connections last one answer. Holding them open would leave node processes
  running for a chat window the user stopped using.
- Tools reach the model as `prefix__tool`, sanitized to `[A-Za-z0-9_-]{1,64}` by
  `namespaced`. A name the provider rejects fails the *whole* request, and the
  prefix is also what stops an external `search_papers` from shadowing ours.
- A server that fails to start is reported in the answer's trail, not swallowed;
  the other servers still load.
- The child gets a widened `PATH` (`augmented_path`): an app launched from the
  Dock inherits only `/usr/bin:/bin:/usr/sbin:/sbin`, so `npx`/`uvx`/`bunx` —
  which is how nearly every MCP server ships — would simply not be found.
- The round-trip is covered by an `#[ignore]`d test that probes Argus's own
  stdio server: `cargo test live_probe -- --ignored`.

When a client silently shows no tools, `claude --debug-file <path> -p hi` prints
the actual validation errors with the offending tool index. Claude Desktop's own
logs report only a bare `result` and reveal nothing.

### Data persistence

The library root contains:

```
<library>/
├── chats/                   # 智能问答 conversations, one JSON file each
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
- Themes are applied via `data-theme` (`system`, `light`, `dark`, `warm`, `forest`, `rose`, `midnight`, `aurora`, `twilight`, `ocean`, `mocha`, `pine`, `sepia`, `mint`, `sky`, `sakura`, `mist`, `peach`). When no `data-theme` is set, the dark palette follows `prefers-color-scheme: dark`. Palettes live in `src/assets/themes.css`; the marketplace metadata (names, preview colors, light/dark kind) lives in `src/utils/themes.ts` — keep the two in sync. Dark themes additionally invert PDF page colors via CSS `filter` rules in `themes.css`.
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
| Add a settings section | `src/components/SettingsModal.vue` + `src/components/settings/`. 智能问答 is a container (`QaSettings.vue`) with Agent and RAG sub-tabs; `initialSection: 'rag'` still routes there |
| Add a sidebar tab | `src/components/RightSidebar.vue` + `src/components/tabs/` |
| Change PDF rendering | `src/components/PdfViewer.vue` |
| Change RAG behavior | `src-tauri/src/rag.rs`, `src/stores/rag.ts`, `src/components/LibraryChat.vue` |
| Change model badges (FREE / 折扣) | `src-tauri/src/llm.rs` (`quotes_free`, `parse_time_discount`, `fetch_openrouter_discount`) → `AiModel` → `stores/ai.ts` → `utils/modelOffers.ts`, rendered in `LibraryChat.vue`, `tabs/AiTab.vue`, `settings/AiSettings.vue`; refreshed by `offer_sync.rs` |

**Where OpenRouter hides its prices.** Three different signals in two different
endpoints, and getting them confused produces badges that are confidently wrong:

- `GET /models` — `pricing.prompt` / `pricing.completion` (`0` both ways = FREE),
  and `pricing.overrides`. That array holds **two opposite things**: entries with
  `utc_start`/`utc_end` are off-peak *discounts*, entries with
  `min_prompt_tokens` are long-context *surcharges* (64 of 414 models carry one,
  every one a price increase). Only the former may be read as a discount.
- `GET /models/{id}/endpoints` — `pricing.discount`, a `0..1` fraction, which is
  the standing promotion. **It is absent from the bulk list entirely** (0 of 414
  entries), which is why the first cut of this feature displayed no promotions
  at all. One request per model, so only `offer_sync` does it, and only for
  models the user actually saved.
- A model is served by several endpoints at different prices *and* different
  discounts. `discount_of_quoted_endpoint` picks the one whose price matches
  what is on screen; taking the best across all of them would advertise a rate
  the user's requests are never billed at.

`parse_param_billions` digs a parameter count out of the naming, then the
description (`550b-a55b` → 550B, the *total* not the active count). It reaches
about a third of a catalogue; closed models never publish it, so the UI falls
back to `~100B` and marks it with `~`. The trap is version numbers — `gpt-5.6`
is not a 5.6B model — which is why `scan_param_size` rejects a digit preceded by
a letter or dot.

There is no bulk source for promotions — `?include=endpoints` is silently
ignored, and `/api/frontend/models` 404s. So the model-picker dialog opens
sorted by free tier immediately and calls `fetch_openrouter_discounts` (fan-out,
concurrency 8, ~8s for 414 models) to fold the rest in and re-sort. The result
is cached in `utils/modelOffers.ts` at *module* scope, since the settings modal
is rebuilt on every open.
| Change AI chat | `src-tauri/src/copilot.rs`, `src-tauri/src/llm.rs`, `src/components/tabs/AiTab.vue` |
| Change canvas | `src/views/CanvasView.vue`, `src/components/canvas/`, `src-tauri/src/canvas*.rs` |
| Change import pipeline | `src/stores/import.ts`, `src-tauri/src/metadata.rs`, `src-tauri/src/url_import.rs` |
| Change themes | `src/assets/themes.css` (palettes), `src/utils/themes.ts` (registry), `src/components/settings/ThemeSettings.vue` (marketplace tab), `src/stores/settings.ts` (apply/preview) |
| Change arXiv inbox | `src/views/ArxivView.vue`, `src/stores/arxiv.ts`, `src-tauri/src/arxiv*.rs` |
| Add an MCP tool | `src-tauri/src/mcp/tools.rs` (the read) + `mcp/server.rs` (declaration + `EXPECTED_TOOLS`) + a dispatch arm in `mcp/agent.rs` |
| Change agent mode | `src-tauri/src/copilot.rs` (the loop), `mcp/client.rs` (external servers), `src/components/settings/AgentSettings.vue`, `src/components/LibraryChat.vue` (the trail) |
| Change embedding map | `src/views/EmbeddingMapView.vue`, `src-tauri/src/rag.rs` |

---

*Last updated: 2026-06-24. Keep this file in sync with major architectural changes.*
