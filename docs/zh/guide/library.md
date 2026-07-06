# 文献库管理

文献库是 Argus 的核心：一个本地文件夹，存放你所有的论文、它们的元数据、笔记、AI对话记录，以及你的整理方式等。

## 导入论文

Argus 支持的不只是 PDF:

- **PDF** —— 拖拽导入，或通过导入按钮导入。
- **电子书** —— EPUB、MOBI、AZW3、FB2、TXT。
- **通过 URL** —— 粘贴链接，Argus 会从 **arXiv**、**bioRxiv**、**ACL Anthology**、**OpenReview**、**AAAI** 或直链 PDF 抓取元数据(以及可获取的 PDF 文件)。

导入时，Argus 可以自动从论文正文以及 **Crossref**、**Semantic Scholar** 等外部来源提取元数据(标题、作者、年份、DOI、arXiv ID、发表处、摘要)。如果提取失败，用户也可通过AI自动提取元数据。

<Media src="media/import_url.mp4" caption="通过 arXiv URL 导入论文" />

## 整理

- **分类** —— 层级式文件夹树；一篇论文可归入嵌套的分类。
- **标签** —— 灵活、跨类目的标签，便于筛选。
- **阅读状态** —— 把论文标记为「未读」「在读」「已读」，并且能够自动检测论文状态，将未读论文设置成在读，已读需要手动设置。
- **元数据编辑** —— 修正或补全任意字段；支持导入引用数和BibTex，导出 BibTeX 等。

## 文件管理

在文献库里可直接重命名、复制或删除论文，并在访达 / 资源管理器中打开论文所在文件夹。每篇论文都是一个自包含的文件夹 —— 不会被锁进某个专有数据库，方便用户备份、迁移和查看。

## 存储方式

每篇论文存放在自己的 `papers/<slug>/` 文件夹下:

```
papers/<slug>/
├── meta.json          # 标题、作者、DOI、arXiv ID、摘要、BibTeX……
├── paper.pdf
├── notes/             # 每篇论文可有多条笔记
├── highlights.json
├── fulltext.txt       # 提取出的全文
├── reading_state.json # 当前页码、滚动位置
└── .status.json       # 阅读状态
```

论文文件夹是**数据源(source of truth)**。文献库索引、全文搜索数据库和向量库都是
Argus 可随时重建的缓存。

## 相关

- [阅读与笔记](/zh/guide/reading)
- [AI 工作流](/zh/guide/ai)
