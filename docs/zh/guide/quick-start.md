# 快速上手

[安装](/zh/guide/installation)好 Argus 之后就可以开始使用了。

<Media src="/media/1783323641583.png" caption="Argus初始界面" />

## 首次运行

1. **选择文献库文件夹。** 首次启动时，Argus 会让你选一个文件夹，用来存放所有论文、笔记和各种设置。所有数据都只保存在这个文件夹里。
2. **导入第一篇论文。** 直接拖入 PDF，或用导入功能添加 PDF、电子书，或通过 URL 导入 (支持arXiv、ACL Anthology、OpenReview、AAAI 等常见的文献网站)。
3. **添加 AI 提供商(可选)。** 打开 **设置 → AI 供应商**，添加提供商(OpenAI、Anthropic、OpenRouter、Ollama,或自定义的 OpenAI 兼容接口)并填入 API 密钥和Base URL。这会解锁元数据提取、AI分析、
   AI对话、ArXiv分析和RAG等众多功能。

## 你的文献库文件夹

数据以纯文件形式存放在你选择的文献库文件夹下，便于备份、迁移和查看:

```
<library>/
├── .argus/            # 配置、缓存、加密的 API 密钥、对话记录
├── papers/<slug>/     # 每篇论文一个文件夹(PDF、笔记、高亮、元数据)
├── canvases/          # 论文关系图谱文件
├── inbox/             # arXiv / bioRxiv 每日收件箱
└── snippets/          # 素材库
```

索引、全文搜索和向量索引都是**可重建的缓存** —— 每个论文文件夹里的文件才是真正的数据源。请对文献库文件夹做好备份。

## 主题与语言

Argus 内置多种主题(浅色、深色、暖色、森林、玫瑰、跟随系统)，并支持 **English** 与 **简体中文**。都可在 **设置** 中切换。
