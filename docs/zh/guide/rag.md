# RAG

Argus 会为你的文献库构建**本地**语义索引，从而实现RAG（Retrieval Augmented Generation）功能，让用户能够对整个论文库进行提问和检索。

## 工作原理

1. **切块(Chunking)** —— 论文全文按段落感知切分为带可配置重叠的文本块。
2. **向量化(Embedding)** —— 使用用户选择的embedding模型(OpenAI、Anthropic 或任意兼容提供商)为每个块生成对应向量。
3. **存储** —— 向量存入文献库文件夹内的本地 SQLite 向量表(`vectors.sqlite`)。除了embedding请求本身，数据不会离开本地。
4. **检索** —— 提问时，Argus 用余弦相似度找出最相关的块,作为上下文喂给模型。


## 使用RAG问答

### 配置RAG参数

在进行RAG提问前，用户需要先配置对应的RAG参数，包括选择embedding模型、块大小、重叠大小等。embedding模型部分只有当配置的AI供应商中存在embedding模型时服务才可用。

<Media src="/media/1783335689002.png" caption="使用RAG之前需要配置对应的参数，包括embedding模型、分块大小和重叠大小等" />

### 构建向量索引

配置好RAG参数后，用户既可以在论文列表这里通过右键菜单栏为单篇论文构建向量索引，也可以在左侧文献分类中通过右键菜单栏构建整个分类下的论文的向量索引。

<Media src="/media/1783335850750.png" caption="构建单篇论文的向量索引" />

<Media src="/media/1783335952643.png" caption="构建整个分类下的论文的向量索引" />

构建向量索引的进度会实时显示在中间栏的上方。

<Media src="/media/1783336073239.png" caption="向量索引的构建进度会实时显示" />

### RAG对话

RAG对话的入口在AI随航智能问答下，点击后即会通过独立窗口显示RAG对话界面，用户可以在此输入问题并进行提问。

<Media src="/media/1783336159510.png" caption="RAG对话入口" />

Argus提供了三种智能问答模型，分别是：

**文献库对话**

仅使用文献库中的文章进行提问。用户可以通过消息输入框的+号按钮添加文献库的文章进行问答。该功能可以帮助用户快速比较不同论文之间的异同。

<Media src="/media/library-chat.mp4" caption="文献库对话有助于同时比较多篇论文之间的异同" />

**文献库RAG对话**

在文献库的内容基础上，结合RAG快速准确的检索用户需要的论文并以此回答用户的问题。Argus在回答用户问题的同时也会提供对应的检索出来的文献供用户参考。

<Media src="/media/library-rag-chat.mp4" caption="文献库RAG 对话可以快速检索用户需要的论文并回答用户问题" />



**素材库对话**

针对用户所记录的素材库进行问答。



## 相关

- [AI工作流](/zh/guide/ai)
- [阅读与笔记](/zh/guide/reading)
- [向量图谱](/zh/guide/embedding-map)
