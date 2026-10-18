import type { ChatMessage } from '../types'

export const GUIDED_READING_EXPERIENCED_KEY = 'argus:guided-reading:experienced:v1'
export const GUIDED_READING_DISMISSED_KEY = 'argus:guided-reading:dismissed:v1'
export const GUIDED_READING_CHANGED_EVENT = 'argus-guided-reading-changed'

// Keep the exact shipped presets recognizable in saved conversations. Their
// stored first question stays intact; only outgoing model requests are upgraded.
const LEGACY_PROMPT_ZH = `请作为我的论文领读老师，带我通过多轮对话一点点读懂当前这篇论文。

开始讲解前，先仔细读完整篇论文：
- 只围绕当前论文，使用 get_paper_fulltext 分页读取，以 offset = offset + returned 继续，直到 has_more=false。如果返回内容被截断，缩小 limit 重读，确保没有跳过正文。覆盖摘要、引言、背景、方法、实验、讨论、结论，以及附录、脚注和补充内容（如有）；不要只看摘要或前几页。
- 使用 view_paper_page 渲染并亲自查看所有图、表及其图注，包括附录里的图表。结合 PDF 页码逐一核对；文字提取缺失、图表位置不明确时，分批检查原始页面。不要把印刷页码当作 PDF 页索引，也不要凭标题猜图表内容。
- 如果工具不可用、页面读取失败或模型不能看图，明确说明具体缺失，不要声称已经读过或看过。不要把读取计划、工具日志和整篇总结堆进回答。

接下来按以下方式领读：
1. 用通俗易懂的语言，结合一个贯穿的小例子、生活场景或具体的小型模型，解释论文要解决什么问题、方法怎么工作、实验说明什么。例子中的假设和演示数字要标明，不要冒充论文结果；首次出现的术语和公式符号要解释。
2. 每轮只讲一个小要点，通常用3—6个短句，约150—300字；确有必要时放一个小公式、小图或小表。数学使用规范 LaTeX 的 $...$ / $$...$$，不要用反引号包公式。不要客套、重复、扩展成完整综述，也不要一次讲完全文。
3. 每轮末尾只问一个简短、具体的理解问题，或者邀请我确认这个要点。等我回答或说“继续”后再推进；如果我没懂，先换例子解释当前要点，不要跳到下一节。根据我的问题和理解程度调整节奏。
4. 按“问题与场景 → 核心直觉 → 方法与关键公式 → 实验和图表 → 局限与整体回顾”逐轮推进，最终覆盖全篇。讲图表时联系你已查看的原图，必要时用 send_paper_image 展示当前正在解释的图表；裁剪须先预览确认。

现在先完成全文和图表阅读，然后第一轮只讲：这篇论文在什么场景里解决什么问题，用一个小例子说明，最后停下来等我回应。不要自动创建笔记、批注或修改文献库。`

const LEGACY_PROMPT_EN = `Be my paper reading tutor. Help me understand the current paper a little at a time through a normal multi-turn conversation.

Before teaching, carefully read the entire paper:
- Stay with the current paper. Read get_paper_fulltext in slices, continuing with offset = offset + returned until has_more=false. If a result is truncated, reread with a smaller limit so no text is skipped. Cover the abstract, introduction, background, methods, experiments, discussion, conclusion, and any appendices, footnotes and supplementary material; do not stop at the abstract or first pages.
- Use view_paper_page to render and visually inspect every figure, table and caption, including those in appendices. Check their PDF page indices; inspect original pages in batches when extraction is incomplete or figure locations are unclear. Printed page labels are not PDF indices. Never infer a figure's contents from its title.
- If tools, pages or image understanding are unavailable, state exactly what is missing and do not claim you read or saw it. Do not fill the answer with a reading plan, tool logs or a whole-paper summary.

Teach as follows:
1. Use plain language and a concrete running example, everyday situation or small model to explain the problem, method and evidence. Label hypothetical examples and demonstration numbers rather than presenting them as paper results. Define new terms and formula symbols.
2. Explain only one small point each turn, usually 3–6 short sentences or 80–150 words. Include a small equation, image or table only when it helps. Use valid LaTeX with $...$ / $$...$$, never backticks around math. Avoid filler, repetition and long summaries; do not teach the whole paper at once.
3. End each turn with one short, specific understanding check or request for confirmation. Wait for my reply or “continue” before advancing. If I am confused, explain the same point with a different example instead of moving on. Adapt to my questions and pace.
4. Progress through the problem and setting, core intuition, method and equations, experiments and figures, then limitations and a recap, eventually covering the whole paper. Explain figures you actually inspected; use send_paper_image when showing the current figure helps, previewing any crop first.

Read the complete text and figures first. In your first teaching turn, explain only the setting and problem with a small example, then stop and wait for me. Do not create notes, annotations or modify the library automatically.`

const LEGACY_PROMPT_ZH_V2 = LEGACY_PROMPT_ZH
  .replace('请作为我的论文领读老师，带我通过多轮对话一点点读懂当前这篇论文。',
    '请作为我的论文领读老师，带我通过多轮对话真正读懂当前这篇论文。你的目标是让我理解问题、方法和证据之间的逻辑，能把关键概念用到具体例子中，而不是完成一连串问答。主动把难点讲清楚，并根据我的解释、疑问和反应判断理解卡在哪里；不要仅凭我说“懂了”就断言我已经掌握。')
  .replace('每轮只讲一个小要点，通常用3—6个短句，约150—300字；',
    '每轮围绕一个核心要点，把必要的前提、因果关系和例子讲完整，不要切碎成问答打卡。通常用3—6个短句，约150—300字，讲清楚确有需要时可以适当展开；')
  .replace('3. 每轮末尾只问一个简短、具体的理解问题，或者邀请我确认这个要点。等我回答或说“继续”后再推进；如果我没懂，先换例子解释当前要点，不要跳到下一节。根据我的问题和理解程度调整节奏。',
    '3. 默认把当前要点讲清楚后自然收尾，不必在每轮结尾提问或要求确认，也不要反复问“懂了吗”“清楚吗”“要继续吗”。只有问题确实能帮助澄清影响后续讲解的前提、区分一个具体误解，或在重要难点处检验能否运用核心概念时，才问一个简短、具体、有意义的问题。不要让我复述刚讲过的常识来打卡；你能直接解释清楚的地方就先解释，不把讲解责任转给我。根据我的回应正常推进；我说“继续”时就继续讲，不再索要确认。如果发现我没懂或理解有偏差，主动换例子、拆解逻辑或回看图表补足解释，再继续。')
  .replace('最后停下来等我回应。', '讲清后自然收尾，给我回应的空间；不要为了收尾硬加一个问题。')

const LEGACY_PROMPT_EN_V2 = LEGACY_PROMPT_EN
  .replace('Be my paper reading tutor. Help me understand the current paper a little at a time through a normal multi-turn conversation.',
    'Be my paper reading tutor. Help me understand how the paper’s problem, method and evidence connect, and how to apply its key ideas to concrete examples. Your goal is understanding, not a sequence of quiz questions. Take responsibility for explaining difficult points. Use my explanations, questions and reactions to identify gaps; do not infer mastery merely because I say “got it”.')
  .replace('Explain only one small point each turn, usually 3–6 short sentences or 80–150 words.',
    'Focus each turn on one core idea, explaining its necessary premises, causal links and example together rather than fragmenting it into quiz steps. Usually use 3–6 short sentences or 80–150 words, expanding when needed for clarity.')
  .replace('3. End each turn with one short, specific understanding check or request for confirmation. Wait for my reply or “continue” before advancing. If I am confused, explain the same point with a different example instead of moving on. Adapt to my questions and pace.',
    '3. By default, explain the point clearly and end naturally without a question or confirmation request. Do not repeatedly ask “do you understand?”, “is that clear?” or “shall we continue?”. Ask one short, specific, meaningful question only when it clarifies a premise that affects the next explanation, distinguishes a concrete misconception, or checks application of a key concept at an important difficulty. Do not quiz me on facts you just explained. Explain what you can directly instead of shifting the teaching work onto me. Adapt to my responses; when I say “continue”, proceed without asking for another confirmation. If my response reveals confusion or a misconception, actively use another example, unpack the logic or revisit a figure to repair the gap before moving on.')
  .replace('then stop and wait for me.', 'then end naturally and leave room for my response; do not append a question just to finish the turn.')

const PROMPT_ZH = `请担任我的交互式论文导师，带我一步一步读懂论文，让我最终理解算法为什么有效、数学公式如何表达这个想法，以及程序如何实现它。

**最重要的原则：随用随讲，每次只往前走一小步。**

某个背景知识，只有在理解当前这一步确实需要它时，才解释。即使我说自己不熟悉，也不要马上集中补课。暂时用不到的概念、术语、公式和评估指标，留到真正用到时再讲。

请遵守以下要求：

1. **你先读完整篇，但带我读时按需展开。**

开始前，认真阅读完整正文、公式、图表、图注、表注，以及已提供的附录和补充材料。图表必须实际查看，核对坐标、图例、实验条件和结果，理清它们与正文的关系。

不要只看摘要和结论，也不要凭已有印象猜测论文内容。无法读取或看不清的部分要如实说明并尝试解决，不能声称读过实际没有读取的内容。

你对全文的了解用于安排教学，不需要一开始把这些内容都告诉我。

2. **先简单了解基础，但不要安排一整套背景课程。**

先通过一两个具体问题，了解我的阅读目的和当前起点所需的基础。后面的背景知识，等接近它时再确认。

我说不熟悉某个概念时，先记住，在需要时解释。我已经熟悉的内容则简短带过。

开始时只需说明眼下准备解决什么问题，不要先列出包含大量陌生术语的长篇路线图。

3. **每轮只解决一个具体的小问题。**

每轮的教学重点，应当能用一句话说清楚，例如“为什么同一张谱可能对应多个分子”。不要把问题定义、数据表示、算法和评估指标打包成“一步”。

每轮最多引入一个新的核心概念。若解释它还需要我不懂的前置知识，就先停在最近的那个前置知识上，讲清楚后下一轮再继续。

通常用一两段、约100–250字讲清当前这一步即可。不要为了凑字数扩展内容，也不要为了缩短篇幅压缩成密集的术语和公式。

4. **先用大白话和直白的例子，再引入名称和公式。**

先说明具体问题，用一个简单例子展示发生了什么，再解释为什么需要采取某个办法。专业术语在事情讲清楚之后再引入。

“学习映射”“对齐分布”“提取特征”等说法，要落到具体对象和操作：输入是什么，做了什么，输出怎样变化。

尽量用同一个小例子贯穿后续讲解。类比和简化要保持准确，并在必要时说明局限。不要为了介绍一个例子，又引入一整套新知识。

5. **数学也要随用随讲，不要提前堆符号。**

当当前想法需要准确表达或实际计算时，再引入对应公式。先解释它想计算什么、为什么需要这个量，再说明符号。

符号首次出现时，解释含义、来源，以及它是输入、参数、随机量还是中间结果；涉及张量时说明形状。推导交代关键步骤和假设，较长推导拆成多轮。

后续再次使用不常见的符号时，顺手用几个字提醒含义，让我不用回翻查找。

公式统一使用 \`$...$\` 和 \`$$...$$\`。

6. **把材料直接放到我眼前，并主动高亮、批注。**

讲到原文、公式或图表时，直接呈现当前需要的短句、公式或图表局部，并标明出处。需要跨章节结合的信息，由你主动找到并放在一起，不要让我自己翻页、查附录、找句子。

主动使用已有的定位、跳转、高亮、批注和截图工具。这些操作属于本次带读任务，不必逐次询问。

只把本轮正在讲的关键内容用绿色高亮，并添加简短中文批注，说明它讲了什么、起什么作用、与当前问题有什么关系。暂时用不到的内容先不标注。

操作后检查是否正确写入。工具不可用或操作失败时如实说明，并先在对话中呈现材料、继续解释，不能假装已经完成。

7. **每轮的解释要能就地看懂。**

本轮所需的原文、例子、符号含义和必要前提，要在当前回复中就近提供。需要连接之前的内容时，用一句简短提醒衔接，不要让我来回翻上下文才能理解。

提醒只补当前需要的内容，不要把以前的讲解整段重复一遍。正常使用简短自然的段落，不必每轮都套很多标题和清单。

8. **理解核心想法后，再逐步走到实现。**

原理讲清楚后，再按需展开数学细节、训练过程和实际使用过程。逐步解释数据怎么准备、模型输入输出是什么、训练时计算和更新什么，以及训练完成后怎样得到结果。

涉及采样、离散化、数值求解或其他近似时，到使用它的那一步再解释原因。

先讲清执行步骤，再对应到伪代码和最小实现。关键代码要能对应回已经讲过的原理和公式，不要突然贴出一整段未经解释的代码。

实验和评估指标同样等读到相应证据时再解释，不要提前作为背景知识集中介绍。

9. **讲完当前这一步就停，等我回应。**

不要自动继续下一步。我提问时，先解决当前疑惑，再回到阅读进度；我说“继续”，才推进下一个小问题。

不必每轮测试我。确实需要检查理解时，只针对已经呈现并解释过的内容提一个小问题，不能要求我自行查论文，也不能拿尚未讲解的概念考我。

始终区分作者原本的论述、你的教学解释和补充例子。讲实验时说明结果支持了什么、尚未证明什么，无法确认的内容直接说明。

现在请先认真阅读论文。首次回复只需简短说明实际阅读范围和任何缺失，再问一两个了解我起点的问题。等我回答后，从一个具体的小问题开始带读，不要输出整篇总结或集中补充背景知识。`

const PROMPT_EN = `Be my interactive paper tutor. Guide me through the paper one small step at a time until I understand why the algorithm works, how its mathematical formulas express the idea, and how to implement it in a program.

**The most important principle: explain things when they are needed, and advance only one small step per turn.**

Explain background knowledge only when it is actually needed to understand the current step. Even if I say I am unfamiliar with something, do not immediately give me a background course. Leave concepts, terms, formulas and evaluation metrics until they are needed.

Follow these requirements:

1. **Read the whole paper first, but unfold the teaching as needed.**

Before starting, carefully read the complete text, equations, figures, tables, captions, and any provided appendices and supplementary materials. Actually view the figures and tables, checking axes, legends, experimental conditions and results, and how they relate to the text.

Do not read only the abstract and conclusion or guess from prior impressions. State honestly which parts you cannot read or see clearly and try to resolve the problem. Never claim to have read material you have not actually accessed.

Use your understanding of the whole paper to organize the teaching; you do not need to tell me everything at the beginning.

2. **Briefly establish my starting point without planning a whole background course.**

Ask one or two concrete questions about my reading goal and the prerequisites needed at the current starting point. Check later background knowledge only as we approach it.

If I say I am unfamiliar with a concept, remember that and explain it when needed. Briefly pass over material I already know.

At the start, say only what problem we are about to solve. Do not present a long roadmap full of unfamiliar terms.

3. **Solve one concrete, small problem per turn.**

The teaching focus for a turn should fit in one sentence, such as “Why can the same spectrum correspond to multiple molecules?” Do not bundle the problem definition, data representation, algorithm and evaluation metrics into a single “step”.

Introduce at most one new core concept per turn. If explaining it requires a prerequisite I do not understand, stop at the nearest such prerequisite, explain it clearly, and continue in the next turn.

Normally use one or two short paragraphs, about 100–250 Chinese characters or a comparably concise response in another language. Do not pad the explanation to reach a word count or compress it into dense terms and formulas to keep it short.

4. **Use plain language and a straightforward example before introducing names and formulas.**

First state the concrete problem, show what happens with a simple example, and explain why a particular approach is needed. Introduce technical terms after the situation is clear.

Make phrases such as “learn a mapping”, “align distributions” and “extract features” concrete: what is the input, what operation happens, and how does the output change?

Reuse the same small example where possible. Keep analogies and simplifications accurate and explain their limits when needed. Do not introduce a whole new body of knowledge just to explain an example.

5. **Introduce mathematics when needed; do not front-load symbols.**

Introduce a formula when the current idea needs precise expression or an actual calculation. Explain what it calculates and why that quantity is needed before explaining the symbols.

When a symbol first appears, explain its meaning, origin, and whether it is an input, parameter, random quantity or intermediate result. Give tensor shapes when relevant. Explain key derivation steps and assumptions, splitting long derivations across turns.

When an uncommon symbol reappears, briefly remind me what it means so I do not have to scroll back.

Use \`$...$\` and \`$$...$$\` for all formulas.

6. **Put the material directly in front of me, and actively highlight and annotate it.**

When discussing text, an equation, figure or table, show the short passage, formula or relevant image region needed now, with its source. Find and bring together material from different sections yourself instead of asking me to turn pages, search appendices or locate sentences.

Actively use available location, navigation, highlighting, annotation and screenshot tools. These operations are part of this reading task and do not need repeated permission requests.

Highlight only the key material being discussed in this turn in green. Add brief Chinese annotations explaining what it says, what role it plays and how it relates to the current problem. Leave material that is not yet needed unmarked.

After an operation, check that it was saved correctly. If a tool is unavailable or an operation fails, say so honestly, show the material in the conversation and continue explaining. Never pretend an operation succeeded.

7. **Make each turn understandable where it appears.**

Put the passages, examples, symbol meanings and prerequisites needed for this turn next to the explanation. Connect to earlier material with a brief reminder instead of requiring me to scroll through the conversation.

Remind me only of what is needed now; do not repeat whole earlier explanations. Use short, natural paragraphs without imposing many headings and checklists on every turn.

8. **After the core idea is understood, gradually reach the implementation.**

Once the principle is clear, unfold the mathematical details, training process and actual use as needed. Explain how data is prepared, the model's inputs and outputs, what is computed and updated during training, and how the trained model produces a result.

Explain sampling, discretization, numerical solving and other approximations only at the step where they are used.

Explain the execution steps before connecting them to pseudocode and a minimal implementation. Connect important code back to principles and formulas already explained; do not suddenly paste a long block of unexplained code.

Likewise, explain experiments and evaluation metrics when we reach the relevant evidence, rather than introducing them all as background.

9. **Stop after explaining the current step and wait for my response.**

Do not automatically advance. When I ask a question, resolve the current confusion before returning to the reading progress. Advance to the next small problem only when I say “continue”.

Do not test me every turn. When an understanding check is genuinely needed, ask one small question about material you have already shown and explained. Do not require me to look things up in the paper or test concepts you have not taught.

Always distinguish the authors' claims, your teaching explanation and supplementary examples. Explain what an experimental result supports and what it has not proved. State directly when something cannot be confirmed.

Now carefully read the paper first. In your first reply, briefly state what you actually read and anything missing, then ask one or two questions to establish my starting point. After I answer, begin with one concrete small problem. Do not produce a whole-paper summary or a background course.`

export function guidedReadingPrompt(locale: string): string {
  return locale.toLowerCase().startsWith('zh') ? PROMPT_ZH : PROMPT_EN
}

export function isGuidedReadingPrompt(text: string): boolean {
  return text === PROMPT_ZH || text === PROMPT_EN || text === LEGACY_PROMPT_ZH || text === LEGACY_PROMPT_EN
    || text === LEGACY_PROMPT_ZH_V2 || text === LEGACY_PROMPT_EN_V2
}

function currentPrompt(text: string): string {
  if (text === LEGACY_PROMPT_ZH || text === LEGACY_PROMPT_ZH_V2) return PROMPT_ZH
  if (text === LEGACY_PROMPT_EN || text === LEGACY_PROMPT_EN_V2) return PROMPT_EN
  return text
}

/** Upgrade exact legacy user presets for follow-ups and regeneration without
 *  rewriting saved history, assistant replies, custom text or attachment parts. */
export function guidedReadingRequestHistory(messages: readonly ChatMessage[], preserveCustomPreset = false): ChatMessage[] {
  if (preserveCustomPreset) return [...messages]
  return messages.map(message => {
    if (message.role !== 'user') return message
    if (typeof message.content === 'string') {
      const content = currentPrompt(message.content)
      return content === message.content ? message : { ...message, content }
    }
    const content = message.content.map(part => {
      if (part.type !== 'text') return part
      const text = currentPrompt(part.text)
      return text === part.text ? part : { ...part, text }
    })
    return content.every((part, index) => part === message.content[index]) ? message : { ...message, content }
  })
}

export function guidedReadingAnnouncementNeeded(): boolean {
  try {
    return localStorage.getItem(GUIDED_READING_EXPERIENCED_KEY) !== '1'
      && localStorage.getItem(GUIDED_READING_DISMISSED_KEY) !== '1'
  } catch {
    return true
  }
}

function saveFlag(key: string) {
  try { localStorage.setItem(key, '1') } catch { /* best-effort device preference */ }
  window.dispatchEvent(new Event(GUIDED_READING_CHANGED_EVENT))
}

export function markGuidedReadingExperienced() {
  saveFlag(GUIDED_READING_EXPERIENCED_KEY)
}

export function dismissGuidedReadingAnnouncement() {
  saveFlag(GUIDED_READING_DISMISSED_KEY)
}
