# RAG

Argus builds a **local** semantic index of your library to power RAG (Retrieval-Augmented
Generation), letting you ask questions and search across your entire paper library.

## How it works

1. **Chunking** — each paper's full text is split into paragraph-aware chunks with
   configurable overlap.
2. **Embedding** — each chunk is embedded with your chosen embedding model (OpenAI,
   Anthropic, or any compatible provider).
3. **Storage** — embeddings are stored in a local SQLite vector table (`vectors.sqlite`)
   inside the library folder. Apart from the embedding requests themselves, your data never
   leaves your machine.
4. **Retrieval** — at query time, Argus finds the most similar chunks by cosine similarity
   and feeds them to the model as context.

## Using RAG Q&A

### Configure RAG settings

Before asking RAG questions, configure the RAG settings — choose an embedding model, chunk
size, overlap, and so on. The embedding-model option is only available when one of your
configured AI providers offers an embedding model.

<Media src="/media/1783335689002.png" caption="Before using RAG, configure its settings — embedding model, chunk size, overlap, and more" />

### Build the vector index

Once RAG is configured, you can build the vector index for a single paper via the
right-click menu in the paper list, or for a whole collection via the right-click menu in
the collection tree on the left.

<Media src="/media/1783335850750.png" caption="Build the vector index for a single paper" />

<Media src="/media/1783335952643.png" caption="Build the vector index for all papers in a collection" />

Indexing progress is shown live at the top of the middle column.

<Media src="/media/1783336073239.png" caption="Vector-indexing progress is shown in real time" />

### RAG chat

RAG chat lives under AI Copilot's Q&A; clicking it opens the RAG chat in its own window,
where you can type and ask your questions.

<Media src="/media/1783336159510.png" caption="Entry point for RAG chat" />

Argus offers three Q&A modes:

**Library chat**

Ask questions using only the papers in your library. Use the **+** button in the message
box to add papers to the conversation. This is great for quickly comparing similarities and
differences across papers.

<Media src="/media/library-chat.mp4" caption="Library chat helps you compare multiple papers side by side" />

**Library RAG chat**

On top of your library's content, RAG quickly and accurately retrieves the papers you need
and answers based on them. Alongside its answer, Argus also shows the retrieved sources for
reference.

<Media src="/media/library-rag-chat.mp4" caption="Library RAG chat quickly retrieves the papers you need and answers your questions" />

**Snippet chat**

Ask questions against the snippets you've collected in your snippet library.

## Related

- [AI Workflows](/guide/ai)
- [Reading & Notes](/guide/reading)
- [Embedding Map](/guide/embedding-map)
