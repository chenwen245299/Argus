# Snippet Library

The snippet library is a collection box separate from your papers — a place to save
excerpts you come across while reading, for later use. You can search your snippets by
keyword, and the AI in [Ask Library](/guide/rag) can look through them too, making it easy
to find and cite material while writing.

<Media src="/media/snippets.mp4" caption="Collect snippets and find them again" />

## What it does

- **Collect on the fly** — save useful quotes, excerpts, and ideas to the snippet library.
- **Find them fast** — search your snippets by keyword in the snippet library; when you
  ask in [Ask Library](/guide/rag), the AI searches the snippet library itself too.
- **No vectorizing needed** — snippets are not embedded, and need no RAG setup. The AI in
  Ask Library finds them with the `search_snippets` tool, a keyword match over each
  snippet's text, note, source-paper title, and tags.
- **Separate from papers** — the snippet library and your paper library stay out of each
  other's way, dedicated to writing, citing, and gathering inspiration.

## How it's stored

Snippets are stored as files under the `snippets/` folder in your library folder — local
and portable, just like your papers.

## Related

- [RAG & Library Q&A](/guide/rag)
- [Embedding Map](/guide/embedding-map)
