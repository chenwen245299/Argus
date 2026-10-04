<!-- From: /Users/qichengwen/My_APP_UI/Argus/AGENTS.md -->
# Argus — Agent Guide

This file is written for AI coding agents. It assumes you know nothing about the project. Read this before making non-trivial changes.

---

## Project overview

**Argus** is a local-first desktop research workspace for academic papers. It bundles PDF reading, note-taking, metadata extraction, arXiv tracking, paper relationship maps, library-wide agent Q&A, embedding-space visualization, and AI-assisted reading into one application.

- **Frontend:** Vue 3 + TypeScript + Vite + Pinia + vue-i18n.
- **Desktop shell:** Tauri v2 (Rust backend, WebKit-based WebView frontend).
- **Target platforms:** macOS (primary) and Windows. Linux is not currently released.
- **Data model:** Everything is stored locally in a user-chosen library folder. The app uses a hybrid of plain JSON/text files, SQLite FTS5 for full-text search, and SQLite vector tables for the embedding map.

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
cd src-tauri && cargo test --lib  # Run the Rust unit tests (~450)
npm run scan:secrets # gitleaks over the whole git history (needs `brew install gitleaks`)
```

### Upgrading Tauri

Tauri refuses to build when an `@tauri-apps/*` npm package and its Rust crate
differ in **major or minor** version, so the two sides have to move together.

Every `@tauri-apps/*` entry in `package.json` is therefore a `~` range pinned to
the minor its crate sits on (`~2.10.1` for `@tauri-apps/plugin-updater` against
`tauri-plugin-updater` 2.10.1, and so on). That is deliberate — a `^` range lets
`npm install` walk to the next minor on its own and break the build. Don't widen
them back.

To take a new Tauri version, move both sides in one commit:

```bash
cargo update -p tauri-plugin-updater      # in src-tauri/, note the new version
# then edit the matching ~range in package.json and refresh the lock:
npm install --package-lock-only
```

Check the pairs with:

```bash
grep -A1 'name = "tauri' src-tauri/Cargo.lock   # crate versions
grep tauri-apps package.json                    # npm ranges
```

CI installs with `npm ci`, so the committed `package-lock.json` is what every
release artifact is built from — on all three platforms.

---

## Architecture

### Window-based view routing

The app uses multiple Tauri windows rather than browser-style routing. `src/App.vue` selects the top-level view by calling `getCurrentWebviewWindow().label`:

| Window label | View rendered | Purpose |
|--------------|---------------|---------|
| `main` | `MainView` | Primary 3-column workspace |
| `arxiv` | `ArxivView` | arXiv / bioRxiv recommendation inbox |
| `canvas` | `CanvasView` | Paper relationship canvas (Vue Flow) |
| `library-chat` | `LibraryChatView` | Library-wide 智能问答 (agent chat) |
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
| `rag.ts` | RAG provider, embedding model, vector store status, collection embed jobs, and the library-wide 同步缺失 / 完整重建 run (kept here so it outlives the settings modal). `MainView` reloads it on the backend's `rag-settings-changed` event, since the embedding map window saves RAG settings through its own settings modal; a mounted `RagSettings.vue` reloads on the same event |
| `arxiv.ts` | arXiv inbox, config, schedule status, analysis |
| `canvas.ts` | Canvas list, current canvas, auto-save |
| `speech.ts` | Read-aloud: configured speech provider/model/options, the speech capabilities from `list_media_capabilities`, `read()` and its state — see *Media generation and read-aloud* |

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
| `rag.rs` | Vector store and embedding storage behind the embedding map (chat does not read it) |
| `ai_manager.rs` | AI provider CRUD and AES-256-GCM API key encryption |
| `llm.rs` | OpenAI-compatible / Anthropic chat, embeddings, OpenRouter, token usage |
| `render.rs` | Rasterises one PDF page to PNG through the bundled PDFium (the reader's path for PDFs with Type 3 fonts, `view_paper_page` for the agent). One dedicated `argus-pdfium` thread owns the binding and a small document cache (4 docs, keyed by path + mtime + size + file identity, released after 60 s idle); PNG is encoded off that thread with the fast lossless encoder. `render_page_image` returns raw PNG bytes (`tauri::ipc::Response`), `render_page_png` is the older base64 twin. Pixels must stay identical across encoder/cache changes — compare decoded pixels, never PNG bytes |
| `media.rs` | The provider-agnostic contract for everything that is not a conversation (image, speech, transcription, sound, music): task kinds, form-as-data (`MediaField` / `MediaModelSpec`), request/result types, dispatch to the adapters, and the "How to add a provider" checklist — see *Media generation and read-aloud* |
| `stepfun_media.rs` / `minimax_media.rs` | The media adapters: StepFun (all six kinds) and MiniMax (speech only). One file per provider; nothing else changes when one is added |
| `ai_summary.rs` | Generate AI paper summaries and abstract extraction |
| `copilot.rs` | Per-paper and library-wide chat, chat history persistence |
| `arxiv.rs` / `arxiv_scheduler.rs` | arXiv/bioRxiv fetching, inbox storage, scheduled catch-up |
| `canvas.rs` / `canvas_enhance.rs` | Canvas CRUD, edge suggestions, auto-layout, export |
| `snippets.rs` | Snippet library CRUD. Snippets are not embedded: the agent finds them with `search_snippets` (`mcp/tools.rs`), a substring match over text, note, source-paper title and tags |
| `token_usage.rs` | Token and USD cost tracking |
| `url_import.rs` | Import from ACL Anthology, OpenReview, arXiv, direct PDF |
| `settings.rs` | `config.json` settings I/O |
| `mcp/` | Read-only MCP server for external agents, run as a stdio subprocess — see below |
| `offer_sync.rs` | Background re-read of model prices on launch, so a withdrawn free tier stops advertising itself |
| `holidays.rs` | China public-holiday calendar (holiday-cn) refreshed in the background into an app-local cache; `get_cn_holidays` + the `cn-holidays-updated` event feed `src/utils/cnHolidays.ts`, which decides DeepSeek's peak window — see the model-badge notes below |
| `highlight_groups.rs` | Derived view that stitches the per-page records of one cross-page PDF selection back into a single highlight (same `created_at` + `text`, >= 2 pages). Read-only consumers (export, MCP, vectorize) call `read_grouped`; the raw read / save path must stay uncollapsed. TS twin: `src/utils/highlightGroups.ts` |
| `highlight_text.rs` | A highlight's display text: the PDF's per-line breaks merged into one paragraph unless `keep_line_breaks` is set (ebook records untouched). `Highlight.text` itself is never rewritten. TS twin: `src/utils/highlightText.ts` — the two must stay identical |
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
| `mcp/agent.rs` | The same tools in-process, for the app's own agent mode, plus app-only declarations kept out of `tools()` and the server (`canvas_edit_tool`) |
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
`PaperIndexEntry::status`, so it costs about the same as one `find_papers` call
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
a pre-built context. It is the only mode now: `LibraryChat.vue` has no
knowledge-source selector and always sends `knowledgeSource: "agent"`,
`plainFallback: true`, and `selectedPaperSlugs` (the conversation's pins).

- `mcp::agent::tools()` / `mcp::agent::call()` expose the tools in-process. The
  dispatch in `mcp/agent.rs` is written by hand because invoking a `ToolRoute`
  needs a `RequestContext<RoleServer>` that only exists inside a live service;
  two tests keep it in sync with the declarations in both directions.
- `llm::stream_with_tools` is OpenAI-compatible only (DeepSeek, OpenRouter,
  Kimi, custom endpoints). `llm::supports_tool_calling` gates it (Anthropic
  protocol, Kimi Code included, and Ollama are out): the library chat answers
  those without tools (see *Plain fallback*), everyone else gets a clear
  message rather than a 400. It streams content deltas live *and* accumulates
  the `delta.tool_calls[i]` fragments, whose `function.arguments` arrive split
  across chunks and must be concatenated by `index` before they parse. Kimi K2
  gets the same fixed thinking/sampling params here as on the plain chat path,
  and its `reasoning_content` is replayed on assistant turns that carried tool
  calls (`llm::replays_reasoning_with_tool_calls` — Kimi K2 only; DeepSeek's
  reasoner has rejected the field on input).
- `chat_with_library_agent` connects the external servers, then runs
  `run_agent_loop`; the split exists so the child processes are torn down on
  every exit path, cancellation included.
- **Plain fallback.** With `plain_fallback`, `commands::chat_with_library`
  checks `copilot::plain_fallback_reason` *before* connecting any MCP server:
  `no_tools` (`supports_tool_calling` false, or `llm::model_declares_no_tools`
  — only StepFun's and OpenRouter's catalogues are trusted for that),
  `web_search` (DeepSeek's search cannot run inside the tool loop), `speech`
  (StepFun spoken reply on a speaking model). A hit goes to
  `copilot::chat_with_library_fallback`, which emits `{event}-agent` phase
  `fallback` (`reason` / `mode` / `papers` / `detail`) for the notice line, then
  runs the tool-free `copilot::chat_with_library` in mode `papers` (the pins'
  full text, also emitted to `{event}-sources`) or `none` (a plain answer) —
  `copilot::fallback_mode`. There is no retrieval mode: the old `library` mode
  (RAG over the vector store) is never sent, though saved conversations still
  carry it and `LibraryChat.vue` renders it neutrally. If `stream_with_tools`
  fails on the *first* round with an error prefixed `llm::TOOLS_REJECTED_PREFIX`,
  the command retries the question as a fallback with reason `rejected` and the
  provider's text as `detail`. The prefix is set only by the deliberately narrow
  `llm::looks_like_tools_rejected` (400/404/422, about tools, *and* saying
  unsupported); later rounds strip it. Nothing is remembered across questions.
  The paper AI panel and canvas chat do not opt in and still fail loudly. The
  fallback is now the only way into the non-agent `copilot::chat_with_library`,
  which knows only `papers` and `none`: the retrieval sources older builds sent
  (`paper-rag`, `paper-rag-loose`, `snippets`) are gone, and a caller still
  naming one is answered as `none` rather than refused (`tool_free_source`).
- **No RAG in chat.** Nothing on the chat path — library chat, paper AI panel,
  canvas chat, the plain fallback — reads the vector store or calls an embedding
  model. The old `semantic_search` tool is gone (the agent tool list and
  `agent_list_builtin_tools` never carry it), and `find_papers`'s `content` is a
  keyword match. Vectors exist for the embedding map only — a deliberate
  decision, so don't wire them back into answers.
- **Pins.** `copilot::pinned_papers_block` renders the pinned slugs (deduped,
  capped at `MAX_PINNED_PAPERS` = 50, missing papers skipped) from the library
  index, never `get_paper` (which reads each full text), and leaves out anything
  that moves while the user works, since it is cached prefix. `join_blocks`
  puts it in the same stable system block as `paper_context_block`, and it is
  also emitted as `{event}-context` with mode `"pinned"` for the sent-context
  banner. `selectedPaperSlugs` on saved conversations predates pins (it held the
  old 文献库论文 selection), so old conversations load it as pins.
  A pin can outlive its paper (deleted, or renamed to a new slug). The stored
  list is never rewritten behind the user's back; instead `LibraryChat.vue`
  sends only the pins still in its paper list (`livePinnedSlugs`), and
  `chat_with_library_fallback` resolves the slugs against the index again
  before picking its mode. The picker's 已固定 tab reports the dead pins and
  can clear them. Its paper list is re-read when the picker opens, on
  `library-updated` (debounced), and when a canvas sends an unknown slug.
- **Pinning from a canvas** (`src/utils/chatPapers.ts`) is a request/response
  over the event bus: the chat acks at once (`argus-chat-add-papers-ack`), then
  replies with `added` / `alreadyPresent` / `overLimit` / `unknown`. The 700 ms
  timeout only covers the ack — it is what tells "chat not open" apart — so the
  chat may re-read its paper list before answering.
- The system prompt is user-editable (设置 → AI 随航 → Agent 与工具, key
  `agent_system_prompt`); blank falls back to `DEFAULT_AGENT_SYSTEM_PROMPT`.
  That default's substantive instruction is **collection-first retrieval**: walk
  `list_collections` → `find_papers(collection_id)` → narrow (`query`, `tag`,
  years, `venue`, `min_citations`, sorting), and treat a whole-library keyword
  `find_papers` as the last resort. Left to itself a model reaches for the
  keyword sweep, which matches titles and ignores the structure the user built
  by hand. `find_papers`'s `content` argument is the keyword full-text search
  for questions about what is *inside* papers. The prompt also tells the model
  that `find_papers` omits abstracts unless it passes `abstract_detail: "full"`
  (`tools::AbstractDetail`: `"full"` or none — the default, and what any other
  value means), so whether abstracts come back is the model's call. Snippets
  are found with `search_snippets`, a plain substring match.
  `agent_system_prompt` is shared with the keepalive —
  the two must send byte-identical system messages or the warmed prefix is not
  the one the next question sends.
- The loop is bounded by `MAX_AGENT_ROUNDS` (500); on hitting it the model gets
  one final tool-less turn to answer with what it has.
- **Tool output is budgeted in tokens, from the model's window.**
  `ContextBudget::for_model` (`copilot.rs`) reads the model's `context_length`
  (`ASSUMED_CONTEXT_TOKENS` = 128k when unset) and derives two caps: one result
  may be `tokens / 4` (`single_result`, floor `MIN_RESULT_TOKENS` = 2 000), all
  results together `tokens / 2` (`transcript`, floor twice that). Tokens are
  estimated per script (`estimate_tokens`: ASCII at four characters to the
  token, anything else at one), so a Chinese result is not under-counted
  fourfold. `truncate_tool_result` cuts an oversized result and appends a note
  telling the model it was cut, so it narrows or pages with `offset`/`limit`;
  `evict_old_tool_results` then replaces the oldest earlier-round results with a
  stub that says to call the tool again (a stub, not a deletion, which would
  orphan its `tool_calls` entry), and the loop emits phase `evicted`.
- A failing tool is fed back as an error string rather than aborting, so the
  model can correct a bad slug or section name on the next round.
- **Usage is summed, not emitted per round.** `stream_with_tools` returns a
  `TurnUsage` instead of emitting one; the loop folds them and calls
  `llm::emit_usage` once at the end. Emitting per round both showed a cost strip
  during the first tool call and reported only that round's figures.
- Progress is emitted on `{event_name}-agent`, phases `servers` / `thinking` /
  `tool` / `result` / `evicted` / `answering` / `limit`, plus `fallback` from the
  fallback path.

**Prompt-cache keepalive (`cache_keepalive.rs`).** An agent turn sends a large
prefix (the system prompt, every tool schema — external servers' too — and the
conversation), and providers with automatic prefix caching bill a repeat of it
at roughly a tenth of the normal rate — but only while the entry is warm;
DeepSeek's expires after ~10 minutes idle. After each agent answer, `chat_with_library_agent` arms a loop that
re-sends the same prefix every 5 minutes with `max_tokens: 1`.

- The warmed prefix is *not* the loop's internal transcript. It is what the next
  question will send: system + the paper-card/pins block + the clean
  user/assistant history + the answer just given. The loop's `tool` messages
  never reappear in a later request, so warming them would refresh a prefix
  nothing asks for. The `tools` array is snapshotted verbatim from the turn
  (external servers included) for the same reason —
  `agent_tool_defs(bridge, vision, canvas_edit)` is shared, and the
  loop and the keepalive must pass it identical flags or they warm a different
  tools block.
- Gated by `is_worthwhile`: DeepSeek always (documented caching, and turn 1 has
  no hit to observe yet), everyone else only once a turn has actually reported
  `cache_hit_tokens > 0`. Against a provider with no cache the ping would be a
  full-price re-read every 5 minutes to save nothing.
- Stops on any of: the `library-chat` window being gone (checked before every
  ping, so a window torn down without front-end cleanup still ends it), an hour
  since the last question, or two consecutive failures.
- Recorded in the usage ledger under source `cache-keepalive`, so this
  background spend is visible rather than folded into the user's own turns.
- User-switchable in 设置 → AI 随航 → Agent 与工具 →「保持上下文缓存」
  (`agent_keep_cache_warm`, default on).
- Status reaches the chat window on the `cache-keepalive` event (`{active, model,
  pings, stopsAtMs, intervalSeconds}` / `{active: false, reason}`), which drives
  the breathing dot on the 工具设置 button under the composer (and its
  counterpart in the conversation list). The status carries a `conversationId` the backend treats as opaque, so
  the indicator lands on the one conversation whose prefix is actually held. `disarm` is silent — `arm` calls it to
  replace its predecessor, and announcing there would blink the badge between
  every question; explicit stops go through `disarm_and_announce`.

**External MCP servers (`mcp/client.rs`).** Users can point agent mode at other
MCP servers, which Argus launches as stdio subprocesses exactly the way Claude
Desktop launches Argus. Configuration lives in the app-data store
(`mcp_external_servers`, `agent_max_rounds`) and is edited in 设置 → AI 随航 →
Agent 与工具.

- Connections last one answer. Holding them open would leave node processes
  running for a chat window the user stopped using.
- Tools reach the model as `prefix__tool`, sanitized to `[A-Za-z0-9_-]{1,64}` by
  `namespaced`. A name the provider rejects fails the *whole* request, and the
  prefix is also what stops an external `find_papers` from shadowing ours.
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

### arXiv batch analysis (`arxiv.rs`)

"AI 分析全部" (`start_analysis`) claims every `pending` *and* `failed` paper
(pending first; a stale `analyzing` counts as pending), runs them through a
worker pool, and puts anything it did not finish back to the status it had.
Built for subscription plans that throttle hard — MiniMax's Token Plan answers
`529 … 整点高峰 … (2064)` around the top of the hour:

- Errors are sorted by `llm::classify_error` into Transient (pause the whole
  batch, 10 s doubling to 5 min, halve concurrency, re-queue the paper a few
  places down), Fatal (bad key, no balance, used-up plan window such as
  MiniMax `2056`: stop at once) and Request (this paper only: `failed`, with
  the reason in `analysis_error`). The batch also stops after 12 minutes with
  no answer or 12 request failures in a row.
- **Throttles never fail a paper.** `llm::is_throttle` (429 / 503 / 529,
  MiniMax `1002` `1041` `2045` `2062` `2064`, rate-limit wording) marks the
  transient errors that only say "slow down"; they never count towards a
  paper's `max_attempts` — only timeouts and 5xx do. Throttling that never lets
  up is left to the stall limit, which reverts rather than fails.
- **MiniMax is paced before it throttles** (`minimax::batch_pacing`): a Token
  Plan key (`sk-cp-…`) keeps at most `PLAN_MAX_IN_FLIGHT` = 3 requests in
  flight (the plan FAQ: about 3–4 agents on Plus, 4–5 on Max, 6–7 on Ultra at
  peak hours; measured, a batch at 4 plus one more request was throttled
  off-peak, so 3 leaves room for the user's chats), and every MiniMax key spaces request starts
  (`BatchTuning::min_interval`) to 75% of the model's published RPM — 200 for
  M3, 500 for the M2 line. The `started` event carries the effective
  `concurrency` and a `concurrency_note` when it was capped.
- **`classify_error` reads markers in the message text** — a MiniMax
  `(NNNN)` code, quota wording, the HTTP status as `(429)` / `API error 529`,
  network wording. When changing `friendly_error` or any error string in
  `llm.rs`, keep those markers; `error_class_tests` guards them.
- Results are buffered and written every 2 s under `inbox_lock`, which every
  inbox read-modify-write takes. Day files that cannot be parsed are never
  written or deleted (`read_day_papers_checked`), and only real
  `YYYY-MM-DD.json` names count as day files — `read_state.json` used to be
  deleted as an "empty day".
- A single-paper analysis registers in `single_in_flight` so a bulk run started
  meanwhile skips that paper; a bulk run refuses single analyses.
- `get_arxiv_schedule_status` carries the current pause, the running batch's
  `run_counts` and the last run's outcome, for a window opened after the
  events went out.
- **A paper scored below the threshold leaves the inbox but is not lost.** It
  goes into `inbox/filtered.json` (newest first, capped at `FILTERED_KEEP` =
  500, analysis included — written in the same `inbox_lock` pass that removes
  it), and so does whatever the list's 刷新 button prunes. The 最近过滤 panel
  (`ArxivFilteredPanel.vue`) reads it; `restore_arxiv_filtered` puts a paper
  back as `done` with `kept: true`, which the threshold never removes again.
  Before this a strict model looked broken: MiniMax-M3 scored 0–3 what others
  scored 6–7, answered in about two seconds, and the batch deleted papers
  several a second with nothing on screen saying why. Every per-paper event
  now carries the run's `succeeded` / `filtered` / `failed` so far, shown next
  to the progress bar and in the cancel/stop notices.
- **Replies are parsed leniently** (`parse_analysis_result`): fields are read
  from a raw JSON map, so a list where a string was asked for (or the other
  way round) is converted rather than failing the paper, and a candidate that
  does not parse goes through `repair_json_strings` once — MiniMax writes
  Chinese quotations with bare ASCII `"` inside the value, which failed that
  paper on every retry. Valid JSON is never rewritten. Field names go through
  `canonical_field` (any case or separators, a few aliases, up to two typos:
  MiniMax-M3 writes `relevence_score`), and a reply that still does not parse
  is read field by field by `salvage_fields`, from each name to the next — M3
  also closes a summary with `"…"]`. Between them these two were 40 of 41
  failures in one run. A parse error that remains carries the text around
  where it broke (`excerpt_at`), not just the reply's first 200 characters.

### PDF page rendering

Two engines draw a page, and **a page's bitmap is always derived from its CSS box**: `cssW = round(viewport.width)`, `pxW = round(cssW × devicePixelRatio)`, pdf.js gets the output transform `[pxW/vp.width,0,0,pxH/vp.height,0,0]`, PDFium gets exactly `pxW × pxH` (never a whole-number DPI), and `(scale, dpr)` is the staleness key. Show a bitmap whose size differs from CSS box × dpr and the browser resamples the whole page — that was the blurry-text bug. A PDF whose bytes contain `/Type3` is drawn by PDFium (`render_page_image`, see `render.rs`) because pdf.js renders those glyphs blank; everything else is a pdf.js canvas. The text, highlight, annotation and link layers are pdf.js's in both cases.

*What gets rendered, and when* is decided by the pure module `src/utils/pageRenderPolicy.ts` (unit-testable without a browser); `PdfViewer.vue` carries the plan out. It replaced an IntersectionObserver with a fixed 600 px margin, which had three real problems: a hidden tab (`v-show` → `display:none`) made every page report "out of view" and **wiped the whole tab**, so each switch back re-rendered everything from white; 600 px is less than one page at 189 % so pages started rendering only when about to be seen; and the same threshold evicted, so pages flapped.

- The render zone is measured in viewports and leans in the direction of travel; a wider keep zone plus a pixel budget (100 MP for the visible viewer, 64 MP for all background tabs together) decides eviction. Pages are rendered by priority (visible, then ahead, then behind) through a queue of two, work that has not started is dropped when its page leaves the zone, running pdf.js tasks are cancelled outside the keep zone, and **a page whose render is in flight is never evicted** (it would finish into an empty wrapper and be marked fresh).
- A hidden viewer plans nothing and keeps its canvases. On show it restores the scroll position first, then re-checks every mounted page against `(scale, dpr)` (a zoom while hidden leaves a stretched stale bitmap otherwise) and renders only what is missing. A tab that finishes loading while hidden defers `fitWidth` / `restorePosition` until it is shown (`scrollTop` cannot be set on a `display:none` box).
- `renderingPages`, `inflightRenders` and the generation counter keep their meaning; never clear `renderingPages`.
- `PDF_ENABLE_HWA` (next to `getDocument`) makes pdf.js use accelerated canvases: far less main-thread compositing per page, text pixel-identical, vector line art antialiased slightly differently. Set it to `false` for bit-identical output with the old software canvases.
- `reader.persistReadingState(rs, slug)` takes the viewer's own slug: the deactivation watcher runs after `activeSlug` has already changed, so defaulting to it saved a tab's position into the next tab.
- Measured with the real viewer in a WKWebView harness: a tab switch used to cost 33–450 ms of white and 3 renders per switch, now 0 and 0.

### Page furniture in cross-page highlights

Dragging from the foot of one page into the next also selects, in DOM order, the footnotes and page number of the first page, its arXiv margin stamp, the running head of the next and any figure or table set at its top, so the highlight painted the page number yellow and stored "…composition. 2In contrast…". `src/utils/pageFurniture.ts` (pure, no dependencies) labels spans as `pageNumber` / `header` / `footer` / `footnote` / `margin` / `float` from their geometry and text, and `planFurnitureDropReport` applies the policy: **only a selection that really crosses a page boundary loses anything**, and only on the sides that face the break (or, on the outer pages, furniture that sits on the wrong side of the user's own start/end because of DOM order — foot page numbers listed first in ACL/EMNLP PDFs); never a side the user started or ended in, never when it would empty the selection. Floats are the exception to "sides": every figure / table the selection runs through goes, on any of its pages, except the one the user's own start or end lies in. A false positive silently drops text that cannot be recovered, so every rule needs several independent signals and answers "not furniture" when unsure.

**Floats** (`classifyFloats`) are found from their caption: `captionHead` wants a label, a number and a delimiter ("Figure 3:", "Fig. 2.", "TABLE IV" over its title, "Algorithm 1 Training" with the label in its own span, "图 3："), and the block must not continue the paragraph above it. The float's text is then walked outwards inside the caption's column — a figure's labels above it (past the picture's white band only small type within its width), a table's rows on the side that holds cells, an algorithm's numbered steps — and every walk stops at running text, a heading, a numbered display, another caption or furniture. Traps found in the corpus: a wrapfigure's caption runs together with the text beside it (continuation lines must be aligned and no wider); the next column's text is not a "piece" of a caption line; a paragraph can open a column with "Figure 1. At the top level, …" (a caption in the text's size must show its float — labels, rows, a picture band ≥ 2.5 em — or be ≤ 4 lines set off from what follows); pdf.js reports CJK faces as monospace (CJK is never "code"); "Table7.Obviously" / "Figures 4a–4d present" are prose.

`planSelectionFurniture` in `PdfViewer.vue` runs at mouse-up, while the selection exists: `popup.text` / `popup.pages` hold the trimmed selection and `popup.full` the whole one (its text rebuilt from the text layers). There is **no toggle** to put the skipped spans back — the user asked for the popup's "已跳过…" button to go (2026-10-04) — so the skipped spans get `.sel-skip` (their `::selection` is transparent) and the page shows what a highlight will take; ⌘C copies the trimmed text (`onCopySelection`); a selection that starts or ends inside furniture of a side, or inside a float, keeps it. A drag released over the previous selection's popup is a new selection, not a click on the popup (`pressInPopup`). Rules for anyone touching it:

- **Geometry comes from pdf.js text content**, not from the DOM: WebKit measures text-layer span widths differently from Chrome, which broke footnote detection in the real WKWebView. `renderPage` keeps `{content, view, rotate}` per rendered page (`pageTextSources`) and `preferContentGeometry` swaps it in when span counts and texts match one to one; the DOM supplies only element identity and line ends.
- **Fail safe**: the text rebuilt from the spans *without* dropping anything must equal `selection.toString()` **ignoring whitespace**, otherwise nothing is trimmed. Never compare line breaks: the app shell is `user-select: none` with only `.textLayer` opting back in (App.vue), so WebKit's `toString()` puts no line break between two pages ("…composition.\n2In contrast") and an exact comparison failed on every cross-page selection — the feature was a silent no-op in the app for that reason while every harness without App.vue's CSS passed. Every selection across pages therefore takes the rebuilt text (`planSelectionFurniture` → `popup.full`), which also fixes words running together across a page break. Test this path only with App.vue's selection rules applied. Records of one highlight must keep sharing the same trimmed `text` and `created_at` (`highlightGroups` groups on that pair).
- The report (`items`, `counts`) describes only spans that are in the selection (nothing in the popup shows it now; keep it that way for any notice that comes back).
- Verified on 6977 pages of 310 PDFs (precision first: no false positive found in the audits; recall roughly 83–95 % depending on the kind). Known misses fail towards the old behaviour. Scanned/OCR pages and `/Rotate` pages are a no-op by design. Floats were audited on the same corpus (7101 pages, ~4100 floats): body text next to a float, tables at body size and every first-page float were checked by hand; simulated cross-page selections (6011 page pairs) drop a float on 1503 of them. Single-page selections are untouched, floats included.

### Media generation and read-aloud

Image generation and editing, speech, transcription, sound design and music are
not chat, and no two providers spell them alike, so they sit behind one
provider-agnostic contract in `media.rs`. **The form is data, not code:** each
adapter *describes* its models and their knobs (`MediaField`: select / number /
toggle / text, with defaults, ranges and notes) and the frontend renders the
form from that — the media studio (`MediaStudioView.vue`) and the read-aloud
settings alike. Adding a provider is one adapter file plus two dispatch lines;
no `.vue` changes.

- **Dispatch.** `media::capabilities(provider)` and `media::run(provider, key,
  req)` pick the adapter with `is_stepfun` / `is_minimax` (by `kind` or base
  URL, like the chat side). `list_media_capabilities` lists only providers that
  are enabled, have an adapter *and* have an API key on file; `run_media_task`
  takes a `MediaRequest` (`providerId`, `kind`, `model`, `prompt`, `inputs`,
  untyped `options` keyed by `MediaField::key`) and returns artifacts as a data
  URI, a hosted URL (these expire) or text. It can be long (music polls for
  minutes). All calls go out from Rust (`reqwest`), so no capability entry.
- **Adapters.** `stepfun_media.rs`: all six kinds; its speech is JSON to
  `/v1/audio/speech` answering with *raw bytes and no envelope*, 1000 characters
  per request. `minimax_media.rs`: speech only, `POST {base_url}/t2a_v2`; see the
  gotchas below.
- **Adding a speech provider** (the same checklist is the module doc of
  `media.rs`): (1) `<provider>_media.rs` with `capabilities()` and `run()`, and
  a `mod` line in `lib.rs`; (2) describe every knob as a `MediaField`, each
  `default` being exactly what `run` does when the key is absent, offering a
  knob only for the models that take it; (3) declare `max_prompt_chars` and
  enforce the same number in `run`, counted in characters; (4) one `is_<provider>`
  arm in `media::capabilities` and one in `media::run`; (5) HTTP errors through
  `llm::friendly_error`, and body-level errors worded so the vendor code stays
  on the end in parentheses (`llm::classify_error` reads it); never log or echo
  the key; (6) tests, including one row in `adapters()` in the `media.rs` test
  module, which holds every adapter to the same form rules (unique keys, a
  select's default is one of its options, a number's default is in range, every
  speech model declares `max_prompt_chars`, no headerless PCM).

**What read-aloud relies on** — the three contracts between backend and
frontend, deliberately small:

1. `MediaModelSpec.max_prompt_chars` (wire: `maxPromptChars`, optional) is the
   provider's *hard* limit on `prompt`, in characters. The player chunks by it
   and may choose smaller chunks, never larger. StepFun speech declares 1000,
   MiniMax 9 999.
2. `AppSettings` gains `speech_provider_id`, `speech_model_id`,
   `speech_options` (keys are the selected model's `MediaField::key`; untyped
   like `MediaRequest::options`) and `speech_skip_citations` (default **true**).
   They live in `.argus/config.json` with the rest of `AppSettings`, so they are
   per library, and `save_settings` writes the whole struct — a frontend path
   that builds settings from anything but the loaded object blanks them. Every
   field is `#[serde(default)]`, so an old file loads unchanged (test), and
   `speech_options` is read leniently (`models::lenient_object`: a `null` or a
   stray array there empties the bag instead of failing the file, which
   `read_settings` would answer by resetting *every* setting). A blank id is
   normalised to `None` in `settings.rs`; `None` is what "not configured" means.
3. Synthesis is `run_media_task` with `kind: "speech"`, `model`, `prompt` = one
   chunk and `options` = `speech_options`; the result is `artifacts[0]` =
   `{mime, dataUrl, filename}`. There is no read-aloud command: it would only
   duplicate the dispatch and the key lookup.

**Read-aloud flow (frontend).** The selection popup's 朗读 button calls
`useSpeechStore().read(text, { source })` synchronously from the click, before
any `await`: the first thing it does is `engine.unlock()`, which starts a silent
clip on the engine's one persistent `Audio` element (insurance for WebViews that
want a user gesture for sound; see the comment on `SILENT_WAV`). Calling it again
with the same text while it is being read stops it, so the button is a toggle.

- `stores/speech.ts` — the configuration (`speech_*` in the app settings, checked
  against `list_media_capabilities`: `isConfigured` / `notConfiguredReason` =
  `unset` | `provider-missing` | `no-providers`), `read` / `stop` / `pause` /
  `resume` / `preview`, and the state (`idle` | `loading` | `playing` | `paused` |
  `error`, `progress`, `errorMessage`, `setupPrompt`, and `isReadingText(text,
  source)` for a popup's stop/read label). `select()` reseeds the
  options from the chosen model's defaults (the adapters' keys differ, see
  *Saved options go stale*); `setOption()` edits one key. Every read freezes its
  provider/model/options (`scopeConfigs`) so a retry or prefetch of an older read
  keeps its voice after the user changes it.
- `utils/speechText.ts` — pure. `prepareSpeechText` merges PDF line breaks
  (`mergeWrappedLines`, the highlight rule) and, when `speech_skip_citations`,
  drops `[12]` / `[1, 2]` / `[3-5]` and author-year (`[Hinton, 2002; LeCun et al.,
  2006]`, `(Du and Mordatch, 2019)`) — and nothing else: the grammar is
  deliberately narrow (a whole bracket must parse as a citation list) and leaves
  `[CLS]`, `(Figure 2)`, `(a)`, `x[1]`, `[0, 1]`, `(Epoch 1500)`, `(CVPR 2019)`
  alone. `chunkForSpeech` cuts at sentence boundaries (never inside `et al.`,
  `Fig.`, decimals, initials), first chunk about 200 characters so audio starts
  fast, later ones up to `min(maxPromptChars, 900)` — deliberately far below
  MiniMax's 9 999, which the docs advise against anyway — counted in code points.
- `utils/speechEngine.ts` — pure, I/O injected, tested under Node with fakes. One
  chunk of lookahead (chunk *i+1* is synthesised while *i* plays, never further),
  a generation token so `stop()` or a new read invalidates everything in flight,
  an LRU cache keyed by `speechCacheKey(scope, text)` (20 chunks / 48 M
  characters; TTS is billed per character, so a replay or a double click costs
  nothing), and one retry for transient errors only (`classifySpeechError`, which
  reads the same markers as `llm::classify_error`: balance, key, length and
  unrecognised errors are fatal and shown with the provider's own words).
- `components/SpeechHost.vue`, mounted once in `MainView.vue` — the floating
  mini-player (z-index 900, under the selection popup at 1000; only the pill takes
  pointer events) and the *not configured* prompt. Its button dispatches
  `argus-open-settings` with `{ section: 'speech' }` (or `'ai'` when no provider
  can speak at all); `SettingsModal.vue` also listens for that event while open,
  because `MainView` only turns it into "show the modal".
- `components/settings/SpeechSettings.vue` (+ `MediaFieldInput.vue`) — 设置 → AI
  随航 → 朗读 (section id `speech`): provider chips, model, one control per
  `MediaField`, the citations toggle and a 试听 button that says it is billed.

If no speech provider/model is set — or the provider has since been deleted,
disabled or lost its key, so it no longer appears in `list_media_capabilities` —
`read()` opens the prompt that points at that settings tab and calls nothing.
Chunking is client-side because the cut points are sentence boundaries of what
the user selected, playback overlaps synthesis, and Stop must be able to cancel
between requests; an adapter handles exactly one request, statelessly, and
enforces the hard limit only as a backstop.

**Gotchas learned the hard way.**

- **MiniMax answers failures with HTTP 200.** The verdict is
  `base_resp.status_code` (0 = fine) and `data` may be `null`;
  `minimax::base_resp_error` reads the envelope and `speech_error` words it.
  Code **1039** is the TPM rate limit on this route but "token limit" in the
  shared error table (right for chat's `max_tokens`), so the speech wording is
  local to `minimax_media.rs` and `minimax::code_class` is untouched. A 1042
  (more than 10 % invisible/illegal characters) is usually text copied out of a
  PDF.
- **MiniMax audio is hex text** in `data.audio`, not base64 (decoded by a small
  tested helper — no new dependency). `output_format: "url"` would give a link
  that expires in 24 h, so bytes are always requested. Only `mp3` / `wav` /
  `flac` are offered: `pcm` and `pcmu_*` are headerless or 8 kHz telephony, and
  `opus` is Ogg, which WebKit's `<audio>` does not reliably play.
- **Limits are characters, not bytes, and billing counts differently.** MiniMax
  takes "less than 10 000" (so 9 999) characters, recommends streaming past
  3 000 (this adapter does not stream, so keep chunks small), and *bills* one
  Chinese character as two. StepFun is 1000 characters.
- **Saved options go stale.** Both adapters call their dropdown `voice`, so
  switching provider leaves the other's id in `speech_options`. The adapter
  trusts nothing: a `voice` outside its list falls back to the default,
  numbers are clamped (a read must not fail because a slider was dragged past
  its end), `emotion` is sent only for models that document it (`fluent` /
  `whisper` only on 2.6; the form's `auto` means *omit the field* and is never
  sent), `language_boost` is checked against the options offered, and a
  Cantonese voice under `auto` becomes `Chinese,Yue`.
- MiniMax reads inline `(…)` in the text as a pronunciation override (and, on
  2.8, `(laughs)`-style tags), so text preparation must not invent parentheses.
- Every read bills per character; the first MiniMax model listed is the cheaper
  Turbo. The default voice is English (`English_Graceful_Lady`) with
  `language_boost: auto`, because the reader works mostly through English
  papers.
- Probing a live key: `ARGUS_MINIMAX_KEY=… cargo test --lib
  minimax_media::tests::live_probe -- --ignored --nocapture` (optionally
  `ARGUS_TTS_OUT=/path/out.mp3` to listen to it).

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
│   ├── vectors.sqlite       # Paper chunk vectors behind the embedding map (a legacy `snippet_chunks` table may linger; nothing reads or writes it)
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
├── inbox/                   # arXiv/bioRxiv daily inbox JSON (YYYY-MM-DD.json), read_state.json, filtered.json (papers filtered out, restorable)
└── snippets/                # Snippet library JSON (never embedded)
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

- **Frontend:** No test runner or test files. `vue-tsc --noEmit` (part of `npm run build`) is the only check.
- **Backend:** Unit tests live in `#[cfg(test)]` modules inside the files they test — 43 of the 61 files under `src-tauri/src/`, about 450 tests (444 run by default; 4 more are `#[ignore]`d — a live MCP probe, a scan timing and two sample dumps, run by hand with `-- --ignored`). The heavier suites guard behavior described elsewhere in this file: `llm.rs` (`error_class_tests`, `provider_error_tests`), `copilot.rs` (context budget, eviction, fallback), `mcp/server.rs` + `mcp/agent.rs` (tool list, schema rules, dispatch sync), `arxiv.rs` (batch analysis).
- **CI:** The release workflow does not run tests.

To run the Rust tests locally:

```bash
cd src-tauri && cargo test --lib
```

When adding significant backend logic, add a `#[cfg(test)]` module in the relevant Rust file.

---

## Security considerations

- **Path traversal:** `path_guard.rs` validates slugs, note IDs, canvas IDs, and library IDs. Do not bypass it when constructing filesystem paths.
- **macOS sandbox:** `security_bookmark.rs` creates and restores security-scoped bookmarks for the library root. Access must be started/stopped with bookmark APIs.
- **API keys:** Encrypted with AES-256-GCM using a per-library random master key. The master key lives in `.argus/.keymaster`.
- **CSP:** `tauri.conf.json` sets `"csp": null`. Be cautious when rendering untrusted HTML/markdown; the frontend already uses DOMPurify.
- **HTTP permissions:** `src-tauri/capabilities/default.json` only allows `https://export.arxiv.org/**` and `https://api.biorxiv.org/**` for built-in fetch. Other HTTP calls go through `tauri-plugin-http` and must be declared in capabilities.
- **URL opening:** `open_url` only permits `http://` and `https://` schemes.
- **Secret scanning (gitleaks):** the repository is public, so a committed key is a leaked key. `.githooks/pre-commit` scans the staged diff and `.githooks/pre-push` every commit about to leave the machine; `.github/workflows/gitleaks.yml` scans each push and PR as the backstop for `--no-verify`. All three read `.gitleaks.toml` (upstream rules plus Argus-specific ones: MiniMax `sk-cp-`, Qwen `sk-sp-`, OpenRouter, Zhipu, a generic `sk-` rule, the Tauri updater private key, and a committed `api_keys.json`/`.keymaster`). `npm install` arms the hooks through `scripts/setup-git-hooks.js` (`core.hooksPath=.githooks`; a no-op in CI, outside a git checkout, or when the developer already set their own `hooksPath`), and the hooks refuse to run without `gitleaks` installed (`brew install gitleaks`) rather than silently passing. A finding that is not a secret gets an inline `gitleaks:allow` comment or its fingerprint in `.gitleaksignore` — never a looser rule. **If a real key was ever committed, rotating it at the provider is the fix; rewriting history does not un-leak it.** Never put real keys in tests or fixtures, and never paste one into a prompt or a doc.

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
| Add a settings section | `src/components/SettingsModal.vue` + `src/components/settings/`. The nav is 常规 / 主题 / AI 供应商 / AI 随航 / MCP 接口 / 关于. AI 随航 (section `agent`) is a container (`QaSettings.vue`) with Agent 与工具 / RAG / 向量化 / 论文分析 / arXiv 爬取 / 朗读 (`speech`) sub-tabs; `initialSection: 'rag'` (the embedding map's button), `'extraction'` and `'arxiv'` still route there |
| Add a sidebar tab | `src/components/RightSidebar.vue` + `src/components/tabs/` |
| Change PDF rendering | `src/components/PdfViewer.vue` + `src/utils/pageRenderPolicy.ts` — read *PDF page rendering* below first; the sharpness invariants are not negotiable |
| Change cross-page highlights / page numbers, footnotes, figures and tables | `src/utils/pageFurniture.ts` (the classifier and the drop policy), `planSelectionFurniture` in `PdfViewer.vue` (the integration), `src/utils/highlightGroups.ts` + `src-tauri/src/highlight_groups.rs` (how the per-page records are shown as one) |
| Change RAG / vectorizing | `src-tauri/src/rag.rs`, `src/stores/rag.ts`, `settings/RagSettings.vue`; vectorizing is papers only and is started from `PaperList.vue` / `LeftSidebar.vue` (collection menu) / 设置 → AI 随航 → RAG / 向量化, not the chat window. Snippets are not vectorized. The vectors serve the embedding map only — chat (agent loop and plain fallback) does not use RAG. The 同步缺失 / 完整重建 run lives in `stores/rag.ts`, not the panel, so it keeps going after the settings modal is closed; `RagSettings.vue` only starts it and shows its progress |
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

**DeepSeek's peak window (波峰 / 波谷) has one implementation**: `isPeakHour` /
`describePeakPeriod` in `src/utils/modelPricing.ts`. The toolbar chip, every
`estimateCostCny` call and `TranslationHistoryTab.vue` use it; never write
another copy of the 9/12/14/18 windows. The official wording (pricing page,
footnote 2): 「北京时间周一至周五（不含中国法定节假日）9:00 - 12:00、14:00 - 18:00
为高峰时段；其余时段，包括周末及中国法定节假日全天均为空闲时段」, off-peak
priced at 0.5x. So peak = a *Beijing* Monday–Friday that is not an official day
off, 09:00 <= t < 12:00 or 14:00 <= t < 18:00, read off the UTC+8 wall clock and
date, never the user's timezone. A weekend is off-peak even on a 调休 working
day (literal reading; DeepSeek's 2026-09-19 note, as the press quotes it, says
the same); a weekday day off, bridge days included, is a holiday, and
`reason: 'holiday'` wins over `'weekend'` (the chip's tooltip says why).
Which days are holidays comes from `src/utils/cnHolidays.ts`, three layers: the
runtime calendar (`holidays.rs` re-reads NateScarlet/holiday-cn, jsDelivr then
raw.githubusercontent.com, for this and next year ~45 s after launch and at
most every 3 days, validates it strictly, caches `cn_holidays.json` in the
app-local data dir, serves it via `get_cn_holidays` and emits
`cn-holidays-updated`), which replaces the embedded 2025/2026 State Council
arrangements (国办发明电〔2024〕12号 / 〔2025〕7号, each date checked against
gov.cn and holiday-cn), and for a year with neither, only the 13 statutory days
(a lunar-date table, because `Intl`'s Chinese calendar differs between engines:
Node 24's ICU is a day out for 春节 2027 and 2030; 清明 by formula) with a one-time `console.warn` — bridge days
cannot be derived. Each Nov/Dec the State Council publishes next year's notice
and holiday-cn follows within days, so the runtime refresh normally makes this a
non-event; to also ship it offline, add the year's rows to `EMBEDDED_ROWS`
(放假 dates `1`, 上班 dates `0`), compare with
`https://cdn.jsdelivr.net/gh/NateScarlet/holiday-cn@master/<year>.json`, and
extend `LUNAR_HOLIDAYS` (HKO conversion tables) when it runs out.
| Change AI chat | `src-tauri/src/copilot.rs`, `src-tauri/src/llm.rs`, `src/components/tabs/AiTab.vue` |
| Change canvas | `src/views/CanvasView.vue`, `src/components/CanvasPanel.vue`, `src/components/canvas/`, `src-tauri/src/canvas*.rs`. Edges are polylines only, never curves: `AdjustableEdge.vue` draws smooth-step until the user places control points, then the orthogonal route from `src/utils/orthogonalRoute.ts`; both hosts set the drag-to-connect line to smooth-step too |
| Change import pipeline | `src/stores/import.ts`, `src-tauri/src/metadata.rs`, `src-tauri/src/url_import.rs` |
| Change themes | `src/assets/themes.css` (palettes), `src/utils/themes.ts` (registry), `src/components/settings/ThemeSettings.vue` (marketplace tab), `src/stores/settings.ts` (apply/preview) |
| Change arXiv inbox | `src/views/ArxivView.vue`, `src/stores/arxiv.ts`, `src-tauri/src/arxiv*.rs` |
| Add an MCP tool | `src-tauri/src/mcp/tools.rs` (the read) + `mcp/server.rs` (declaration + `EXPECTED_TOOLS`) + a dispatch arm in `mcp/agent.rs` |
| Change agent mode | `src-tauri/src/copilot.rs` (the loop), `mcp/client.rs` (external servers), `src/components/settings/AgentSettings.vue`, `src/components/LibraryChat.vue` (the trail, pins, fallback notice) |
| Change embedding map | `src/views/EmbeddingMapView.vue`, `src-tauri/src/rag.rs` |
| Add a media / text-to-speech provider | A new `src-tauri/src/<provider>_media.rs` + two dispatch lines in `media.rs` + a row in the `adapters()` test helper there; follow the checklist in the `media.rs` module doc. No UI change: the studio and 设置 → AI 随航 → 朗读 render from the description |
| Change read-aloud | `src/stores/speech.ts`, `src/utils/speechText.ts` (chunking), `src/utils/speechEngine.ts` (playback), `src/components/SpeechHost.vue`, `src/components/settings/SpeechSettings.vue`; backend side `src-tauri/src/minimax_media.rs` / `stepfun_media.rs` |

---

*Last updated: 2026-10-04. Keep this file in sync with major architectural changes.*
