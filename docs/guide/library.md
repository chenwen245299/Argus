# Literature Library

The library is the heart of Argus: a local folder that holds all your papers, their
metadata, notes, AI chat history, and how you've organized them.

## Importing papers

Argus accepts more than just PDFs:

- **PDF** — drag and drop, or import via the import button.
- **Ebooks** — EPUB, MOBI, AZW3, FB2, and TXT.
- **By URL** — paste a link and Argus fetches metadata (and the PDF where available) from
  **arXiv**, **bioRxiv**, **ACL Anthology**, **OpenReview**, **AAAI**, or a direct PDF URL.

On import, Argus can automatically extract metadata (title, authors, year, DOI, arXiv ID,
venue, abstract) from the paper text and from external sources like **Crossref** and
**Semantic Scholar**. If extraction fails, you can also auto-extract metadata with AI.

<Media src="/media/import_url.mp4" caption="Importing a paper from an arXiv URL" />

## Organizing

- **Collections** — a hierarchical folder tree; a paper can live in nested collections.
- **Tags** — flexible, cross-cutting labels for filtering.
- **Reading status** — mark papers as *unread*, *reading*, or *read*. Argus can also
  auto-detect status and move unread papers to *reading*; *read* is set manually.
- **Metadata editing** — fix or complete any field; import citation counts and BibTeX,
  export BibTeX, and more.

## File management

Right from the library you can rename, duplicate, or delete papers, and open a paper's
folder in Finder/Explorer. Each paper is a self-contained folder — nothing is locked
inside a proprietary database, making it easy to back up, migrate, and inspect.

## How it's stored

Each paper lives in its own folder under `papers/<slug>/`:

```
papers/<slug>/
├── meta.json          # title, authors, DOI, arXiv ID, abstract, BibTeX …
├── paper.pdf
├── notes/             # multiple notes per paper
├── highlights.json
├── fulltext.txt       # extracted full text
├── reading_state.json # current page, scroll position
└── .status.json       # reading status
```

The paper folders are the **source of truth**. The library index, full-text search
database, and vector store are caches Argus can rebuild at any time.

## Related

- [Reading & Notes](/guide/reading)
- [AI Workflows](/guide/ai)
