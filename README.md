<p align="center">
  <img src="./src-tauri/icons/icon.png" width="88" alt="Argus icon" />
</p>

<h1 align="center">Argus</h1>

<p align="center">
  <strong>A lightweight, AI-native literature reader for reading, analyzing, and chatting with your papers.</strong>
</p>

<p align="center">
  English
  ·
  <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="#core-features">Core Features</a>
  ·
  <a href="#development">Development</a>
  ·
  <a href="#status">Status</a>
</p>

> [!CAUTION]
> **Most of this project was generated or heavily assisted by AI.**
>
> Argus is still being actively debugged and improved. Please use it with care and keep backups of your literature library.

<p align="center">
  <img src="./docs/public/main.png" width="900" alt="Argus screenshot placeholder" />
</p>

Argus is a desktop app for managing academic papers and working with them through AI. It brings PDF reading, metadata extraction, notes, arXiv tracking, paper relationship maps, and library-level Q&A into one local-first workflow.

## Core Features

| Feature | What it does |
| --- | --- |
| Literature library | Import PDFs, organize folders, tag papers, track reading status, and keep paper metadata in one place. |
| Reading and notes | Read PDFs in tabs, write notes, manage annotations, and copy abstracts or extracted full text quickly. |
| AI workflows | Extract metadata, extract abstracts from the original paper text, and generate configurable paper analysis. |
| arXiv and RAG | Track arXiv recommendations, import useful papers, build semantic search, and ask questions across the library with sources. |
| Research canvas | Arrange papers as nodes, connect related work, and export paper relationship maps. |

## Installation (macOS)

Download the `.dmg` from the [Releases](../../releases) page, install the app, then run the following command to clear the quarantine flag — otherwise macOS will block it from opening:

```bash
xattr -cr /Applications/Argus.app
```

## Development

```bash
npm run build
npm run tauri build
```

## Status

Argus is experimental and under active development. Expect rough edges, UI changes, and occasional incorrect AI output while the app is being debugged and improved.
