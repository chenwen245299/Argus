# RAG & Library Q&A

This page covers two separate things:

- **RAG / vectorizing** — Argus chunks your papers and embeds them with the embedding model
  you choose, building a **local** vector index inside your library folder. The
  [Embedding Map](/guide/embedding-map) is drawn from that index.
- **Ask Library** — ask questions across your whole library, and the AI looks up papers,
  notes, and snippets itself with its tools. Ask Library does not use the vector index, and
  works without RAG being set up.

## How it works

1. **Chunking** — each paper's full text is split into paragraph-aware chunks with
   configurable overlap.
2. **Embedding** — each chunk is embedded with your chosen embedding model, from OpenAI,
   any OpenAI-compatible provider (such as OpenRouter), or a local Ollama model. Anthropic
   offers no embeddings endpoint, so it can't be used for vectorizing.
3. **Storage** — embeddings are stored in a local SQLite vector table (`vectors.sqlite`)
   inside the library folder. Apart from the embedding requests themselves, your data never
   leaves your machine.
4. **Plotting** — the [Embedding Map](/guide/embedding-map) projects these vectors onto a
   2-D plane, so papers on similar topics land near each other.

## Setting up RAG

### Configure RAG settings

Before using RAG, configure it under **Settings → AI Copilot → RAG / Vectorize** — choose an
embedding model, chunk size, overlap, and so on. The embedding-model option is only
available when one of your configured AI providers offers an embedding model.

<Media src="/media/1783335689002.png" caption="Before using RAG, configure its settings — embedding model, chunk size, overlap, and more" />

### Build the vector index

Once RAG is configured, you can build the vector index for a single paper via the
right-click menu in the paper list (or select several papers, right-click, and choose
**Add to Vector Library**), or for a whole collection via the right-click menu in the
collection tree on the left.

<Media src="/media/1783335850750.png" caption="Build the vector index for a single paper" />

<Media src="/media/1783335952643.png" caption="Build the vector index for all papers in a collection" />

Indexing progress is shown live at the top of the middle column.

<Media src="/media/1783336073239.png" caption="Vector-indexing progress is shown in real time" />

To index the whole library at once, open **Settings → AI Copilot → RAG / Vectorize** and click
**Sync Missing** — it indexes every paper that isn't indexed yet, and if it stops partway,
clicking it again picks up where it left off. After switching embedding models, click
**Full Rebuild** to re-index every paper.

Vectorizing covers papers only, and only happens in these places. The snippet library is
not vectorized; the Ask Library window doesn't have a vectorize button either, and Ask
Library doesn't need your papers vectorized.

## Ask Library

Open it from **AI Copilot → Ask Library**; it opens in its own window, where you can type and
ask your questions.

<Media src="/media/1783336159510.png" caption="Entry point for Ask Library" />

Ask Library has a single mode — there's no knowledge source to pick. Just ask, and the AI
decides what it needs to look up:

- **It looks things up itself.** The model uses Argus's library tools to browse your
  collections, find papers, read their full text, notes, and highlights, look at PDF pages
  (with a vision model), search your snippet library, and open your canvases and past
  conversations — plus any external MCP servers you've connected. It searches paper text by
  keyword, and finds snippets with the `search_snippets` tool, a keyword match over each
  snippet's text, note, source-paper title, and tags; neither relies on the vector index.
  Each lookup is shown with the answer.
- **Questions unrelated to your library** — translation, writing, general knowledge — are
  answered directly, without looking anything up.

<Media src="/media/library-rag-chat.mp4" caption="Ask a question across your whole library" />

The **工具设置** (Tool settings) button below the message box sets the tool-call limit and
which MCP servers to connect. While the conversation's prompt cache is being kept warm, a
breathing dot shows on this button.

Conversations from earlier versions are kept, along with their saved sources and paper tags.

### Pinned papers

To keep a conversation focused on particular papers, click the pin button below the message
box, next to **工具设置**. In the **固定文献** (Pinned papers) dialog, search by title, author,
or year and click papers in the **未固定** (Not pinned) tab to pin them; the **已固定**
(Pinned) tab lists what's pinned — click a paper there to unpin it.

Once papers are pinned, the title bar shows **已固定 N 篇** (N pinned) with an edit button for
changing them. From then on, the model treats every question as being about the pinned
papers and reads them itself — great for quickly comparing similarities and differences
across papers. Pins belong to the conversation, up to 50 papers each. If a pinned paper is
later deleted or renamed, it is no longer sent to the model; the **已固定** tab says how many
pins are no longer in the library and lets you remove them in one click.

You can also pin from a canvas: right-click a paper node (or a box selection) and choose
**固定到智能问答** (Pin to Ask Library). The papers are pinned to the conversation that's open
in Ask Library, so open that window first.

<Media src="/media/library-chat.mp4" caption="Compare several papers side by side in one conversation" />

### Models without tool calling

When a question can't go through the library tools, Argus answers it as a plain chat and adds
a notice line to the answer explaining why. This happens when:

- the model doesn't support tool calling — Anthropic-format providers, Kimi Code, Ollama's
  native API, models whose catalogue lists no tool support (such as StepFun's
  step-audio-r1.5) — or the provider refuses the tools;
- DeepSeek's web search is on — it can't run together with the library tools, so turn web
  search off if you want the model to search your library;
- spoken replies (语音回复) are on for a StepFun speaking model.

In these answers, the model gets your pinned papers' content if the conversation has any,
and those papers are listed in the answer's sources; otherwise it answers directly, without
reading your library. To let the model look through your library and snippets itself,
switch to a model that supports tool calling (for example DeepSeek, Qwen, or most models on
OpenRouter).

## Related

- [AI Workflows](/guide/ai)
- [Reading & Notes](/guide/reading)
- [Embedding Map](/guide/embedding-map)
