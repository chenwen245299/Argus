<p align="center">
  <img src="./image/README/main-intro.png" width="900" alt="Argus" />
</p>

<p align="center">
  <strong>A lightweight, AI-native, local-first literature reader for reading, analyzing, searching, and chatting with your papers.</strong>
</p>

<p align="center">
  English
  ·
  <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="https://chenwen245299.github.io/Argus/">Website</a>
  ·
  <a href="https://chenwen245299.github.io/Argus/guide/introduction">Documentation</a>
  ·
  <a href="https://chenwen245299.github.io/Argus/download">Download</a>
  ·
  <a href="#installation-macos">Installation</a>
</p>

> [!CAUTION]
> **Most of this project was generated or heavily assisted by AI.** Argus is now fairly stable — I've been using it heavily on macOS 26 for a while, fixed most of the bugs, and it runs smoothly there. Basic features also work on Windows.

<p align="center">
  <img src="./docs/public/main.png" width="900" alt="Argus screenshot" />
</p>

Argus is a lightweight, AI-native, **local-first** literature reader for reading, analyzing, searching, and chatting with your academic papers — all from one desktop app. It brings PDF and ebook reading, AI features, note-taking, arXiv tracking, paper relationship maps, a snippet library, and library-wide RAG search and paper analysis together, so your whole research reading workflow lives in one place and blends seamlessly with AI.

> 📖 Full documentation, screenshots, and demo videos: https://chenwen245299.github.io/Argus/guide/introduction

## Design principles

- **Local-first** — your PDFs, notes, highlights, chat history, and AI analysis all live in a library folder **you** choose, on **your** machine. No cloud lock-in, no account required, and your data is easy to migrate.
- **AI-native** — AI isn't a bolt-on feature and doesn't need extra plugins; it's built directly into the app as a native capability. Use it freely to analyze, explain, and summarize papers, and to answer any question. Works with common provider APIs (OpenAI, Anthropic, OpenRouter, Ollama, or any OpenAI-compatible endpoint).
- **Lightweight** — built with [Tauri](https://tauri.app/), so it installs small (~20 MB) and starts fast.

## Core features

| Area | What it does |
| --- | --- |
| [Literature Library](https://chenwen245299.github.io/Argus/guide/library) | Import PDFs and ebooks, organize with folders and tags, track reading status, keep metadata in one place. |
| [Reading & Notes](https://chenwen245299.github.io/Argus/guide/reading) | Read PDFs in tabs, highlight and annotate, and write rich notes with an OCR-backed full-text pipeline. |
| [AI Workflows](https://chenwen245299.github.io/Argus/guide/ai) | Bring any provider and model to extract metadata and abstracts, generate configurable analysis, and chat with any paper. |
| [Canvas](https://chenwen245299.github.io/Argus/guide/canvas) | Arrange papers as nodes, connect related work, trace how a field developed, and export relationship maps. |
| [Snippet Library](https://chenwen245299.github.io/Argus/guide/snippets) | Collect excerpts as you read and search them semantically — handy for finding and citing while writing. |
| [Embedding Map](https://chenwen245299.github.io/Argus/guide/embedding-map) | Project your library's embeddings onto a 2-D map to visualize clusters and discover connections. |
| [arXiv & bioRxiv Tracking](https://chenwen245299.github.io/Argus/guide/arxiv) | Auto-fetch preprints on a schedule, filter by AI relevance, triage from a daily inbox. |
| [RAG & Library Q&A](https://chenwen245299.github.io/Argus/guide/rag) | Build a local vector index and ask questions across your whole library — with sources. |

## Installation (macOS)

Download the `.dmg` from the [Releases](../../releases) page (or the [Download page](https://chenwen245299.github.io/Argus/download)), install the app, then clear the quarantine flag — otherwise macOS will block it from opening:

```bash
xattr -cr /Applications/Argus.app
```

See [Installation](https://chenwen245299.github.io/Argus/guide/installation) for details and Windows notes.

## Development

```bash
npm install
npm run tauri dev      # run the desktop app in dev mode
npm run tauri build    # build the installer
```

## Who it's for

Argus is built mainly for **researchers, PhD students, and anyone managing a growing library of papers while writing their own** — people who want a lightweight, AI-native app to help them read, analyze, and organize their literature.

## Status

Argus is fairly stable on macOS 26 (heavily used, most bugs fixed), and basic features also work on Windows. It's still improving, so expect occasional rough edges and always keep backups of your library.
