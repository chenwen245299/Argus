# Reading & Notes

Argus is first and foremost a reader for papers and books. It's designed to keep you
focused while reading, annotating, and writing — and to let you summon AI for reading help
and questions at any time.

<Media src="/media/reading.mp4" caption="Reading in tabs, highlighting, and taking notes side by side" />

## The reader

- **Tabbed PDF viewer** — powered by pdf.js; open multiple papers at once. Full-text
  search within a document, page navigation, and persistent reading state (Argus remembers
  each paper's page and scroll position).
- **Ebook reader** — read EPUB, MOBI, AZW3, FB2, and TXT with chapter navigation and a
  table of contents.

## Highlights & annotations

Select text to highlight it; highlights are saved per paper in `highlights.json` and stay
with the paper folder.

## Notes

Write as many notes as you want per paper, using a rich Markdown editor (Vditor) with:

- **Math** via KaTeX
- **Diagrams** via Mermaid
- **Code** with syntax highlighting

Notes can open in their own window so you can write next to the paper you're reading —
click the button to the right of a note's title to pop it out, as shown below:

<Media src="/media/1783327859815.png" caption="Click the button to the right of the note title to open it in its own window" />

<Media src="/media/notes.mp4" caption="Writing a note with math and a Mermaid diagram" />

## Full-text extraction

To power search, AI analysis, AI chat, and RAG, Argus extracts each paper's full text
through a layered pipeline:

1. Native PDF text extraction (the fast path).
2. `pdftotext` for tricky PDFs.
3. **OCR** fallback for scanned documents — using the **macOS Vision** framework natively,
   or **Tesseract** elsewhere.

The extracted text is stored as `fulltext.txt` in the paper folder and can be re-extracted
at any time.

<Media src="/media/1783328013050.png" caption="Trigger full-text extraction from the right-click menu" />

## Sections

Argus can auto-split a paper into logical sections (Abstract, Introduction, Methods, …),
making long papers easier to navigate and giving the AI shorter, structured context.

<Media src="/media/1783328174631.png" caption="Argus auto-splits papers into sections" />

## Translation

Before using translation, configure an AI provider and a default translation model. If no
translation model is set, translation falls back to the global default model. We recommend
setting a default model after adding your provider:

<Media src="/media/1783328540768.png" caption="Click the ⭐️ to set the default model" />

Built-in translation renders results in the same format as AI chat — showing the model
used, input/output tokens, and cost — with quick actions to regenerate or copy, plus a
translation history.

<Media src="/media/translation.mp4" caption="Select-to-translate, and browse translation history" />

## Related

- [AI Workflows](/guide/ai)
- [RAG & Library Q&A](/guide/rag)
