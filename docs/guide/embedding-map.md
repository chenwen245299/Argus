# Embedding Map

Argus can project your library's embeddings onto a 2-D map, so you can **see** clusters of
related work and discover connections you might have missed. Open it from **AI Copilot →
Embedding Map**. As shown below, LLM papers and CV papers are automatically separated into
two distinct clusters.

<Media src="/media/embedding-map.mp4" caption="Visualize the relationships between papers in your library via their embeddings" />

## What it does

- **Cluster visualization** — papers on similar topics naturally group together, so you
  can see the themes in your library at a glance.
- **Discover connections** — nearby points tend to be related, making it easy to trace
  your way to adjacent work.
- **Click to locate** — click a point on the map to jump to that paper.

## Prerequisite

The embedding map is built on the RAG vector index. Before using it, build the embedding
index for your library on the [RAG & Library Q&A](/guide/rag) page (configure an embedding
model and index your library).

## Related

- [RAG & Library Q&A](/guide/rag)
