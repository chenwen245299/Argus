# Quick Start

Once Argus is [installed](/guide/installation), you're ready to go.

<Media src="/media/1783323641583.png" caption="The Argus main window on first launch" />

## First run

1. **Choose a library folder.** On first launch, Argus asks you to pick a folder where all
   your papers, notes, and settings are stored. All your data stays inside this folder.
2. **Import your first paper.** Drag a PDF in, or use the import options to add PDFs and
   ebooks, or import by URL (arXiv, ACL Anthology, OpenReview, AAAI, and other common paper
   sites are supported).
3. **Add an AI provider (optional).** Open **Settings → AI Providers** and add a provider
   (OpenAI, Anthropic, OpenRouter, Ollama, or a custom OpenAI-compatible endpoint) with your
   API key and Base URL. This unlocks metadata extraction, AI analysis, AI chat, arXiv
   analysis, RAG, and much more.

## Your library folder

Your data is stored as plain files under the library folder you chose, so it's easy to back
up, migrate, and inspect:

```
<library>/
├── .argus/            # config, caches, encrypted API keys, chat history
├── papers/<slug>/     # one folder per paper (PDF, notes, highlights, metadata)
├── canvases/          # paper relationship map files
├── inbox/             # arXiv / bioRxiv daily inbox
└── snippets/          # snippet library
```

The `index`, full-text search, and vector indexes are **rebuildable caches** — the files in
each paper folder are the source of truth. Keep backups of the library folder.

## Themes & language

Argus ships with multiple themes (light, dark, warm, forest, rose, and system) and supports
**English** and **简体中文**. Change both under **Settings**.
