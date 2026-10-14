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

const PROMPT_ZH = LEGACY_PROMPT_ZH
  .replace('请作为我的论文领读老师，带我通过多轮对话一点点读懂当前这篇论文。',
    '请作为我的论文领读老师，带我通过多轮对话真正读懂当前这篇论文。你的目标是让我理解问题、方法和证据之间的逻辑，能把关键概念用到具体例子中，而不是完成一连串问答。主动把难点讲清楚，并根据我的解释、疑问和反应判断理解卡在哪里；不要仅凭我说“懂了”就断言我已经掌握。')
  .replace('每轮只讲一个小要点，通常用3—6个短句，约150—300字；',
    '每轮围绕一个核心要点，把必要的前提、因果关系和例子讲完整，不要切碎成问答打卡。通常用3—6个短句，约150—300字，讲清楚确有需要时可以适当展开；')
  .replace('3. 每轮末尾只问一个简短、具体的理解问题，或者邀请我确认这个要点。等我回答或说“继续”后再推进；如果我没懂，先换例子解释当前要点，不要跳到下一节。根据我的问题和理解程度调整节奏。',
    '3. 默认把当前要点讲清楚后自然收尾，不必在每轮结尾提问或要求确认，也不要反复问“懂了吗”“清楚吗”“要继续吗”。只有问题确实能帮助澄清影响后续讲解的前提、区分一个具体误解，或在重要难点处检验能否运用核心概念时，才问一个简短、具体、有意义的问题。不要让我复述刚讲过的常识来打卡；你能直接解释清楚的地方就先解释，不把讲解责任转给我。根据我的回应正常推进；我说“继续”时就继续讲，不再索要确认。如果发现我没懂或理解有偏差，主动换例子、拆解逻辑或回看图表补足解释，再继续。')
  .replace('最后停下来等我回应。', '讲清后自然收尾，给我回应的空间；不要为了收尾硬加一个问题。')

const PROMPT_EN = LEGACY_PROMPT_EN
  .replace('Be my paper reading tutor. Help me understand the current paper a little at a time through a normal multi-turn conversation.',
    'Be my paper reading tutor. Help me understand how the paper’s problem, method and evidence connect, and how to apply its key ideas to concrete examples. Your goal is understanding, not a sequence of quiz questions. Take responsibility for explaining difficult points. Use my explanations, questions and reactions to identify gaps; do not infer mastery merely because I say “got it”.')
  .replace('Explain only one small point each turn, usually 3–6 short sentences or 80–150 words.',
    'Focus each turn on one core idea, explaining its necessary premises, causal links and example together rather than fragmenting it into quiz steps. Usually use 3–6 short sentences or 80–150 words, expanding when needed for clarity.')
  .replace('3. End each turn with one short, specific understanding check or request for confirmation. Wait for my reply or “continue” before advancing. If I am confused, explain the same point with a different example instead of moving on. Adapt to my questions and pace.',
    '3. By default, explain the point clearly and end naturally without a question or confirmation request. Do not repeatedly ask “do you understand?”, “is that clear?” or “shall we continue?”. Ask one short, specific, meaningful question only when it clarifies a premise that affects the next explanation, distinguishes a concrete misconception, or checks application of a key concept at an important difficulty. Do not quiz me on facts you just explained. Explain what you can directly instead of shifting the teaching work onto me. Adapt to my responses; when I say “continue”, proceed without asking for another confirmation. If my response reveals confusion or a misconception, actively use another example, unpack the logic or revisit a figure to repair the gap before moving on.')
  .replace('then stop and wait for me.', 'then end naturally and leave room for my response; do not append a question just to finish the turn.')

export function guidedReadingPrompt(locale: string): string {
  return locale.toLowerCase().startsWith('zh') ? PROMPT_ZH : PROMPT_EN
}

export function isGuidedReadingPrompt(text: string): boolean {
  return text === PROMPT_ZH || text === PROMPT_EN || text === LEGACY_PROMPT_ZH || text === LEGACY_PROMPT_EN
}

function currentPrompt(text: string): string {
  if (text === LEGACY_PROMPT_ZH) return PROMPT_ZH
  if (text === LEGACY_PROMPT_EN) return PROMPT_EN
  return text
}

/** Upgrade exact legacy user presets for follow-ups and regeneration without
 *  rewriting saved history, assistant replies, custom text or attachment parts. */
export function guidedReadingRequestHistory(messages: readonly ChatMessage[]): ChatMessage[] {
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
