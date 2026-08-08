<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, nextTick, watch } from 'vue'
import { Icon } from '@iconify/vue'
import type VditorType from 'vditor'

const props = defineProps<{
  initialContent: string
  /** Persist a pasted image and return the URL to render it by. Without this,
   *  Vditor falls back to inlining the file as a base64 `data:` URI. */
  imageUploader?: (file: File) => Promise<string | null>
}>()
const emit = defineEmits<{ change: [markdown: string] }>()

const containerEl = ref<HTMLDivElement | null>(null)
let vd: VditorType | null = null
let destroyed = false

function normalizeDisplayMathDelimiters(markdown: string) {
  const blocks: string[] = []
  const placeholder = (prefix: string, index: number) => `\u0000${prefix}_${index}\u0000`
  let text = markdown.replace(/(```[\s\S]*?```|~~~[\s\S]*?~~~)/g, (match) => {
    blocks.push(match)
    return placeholder('CODE_BLOCK', blocks.length - 1)
  })

  const inlineCodes: string[] = []
  text = text.replace(/`[^`\n]*`/g, (match) => {
    inlineCodes.push(match)
    return placeholder('INLINE_CODE', inlineCodes.length - 1)
  })

  text = text.replace(/\\\[([\s\S]+?)\\\]/g, (match, tex: string) => {
    const body = tex.trim()
    return body ? `$$\n${body}\n$$` : match
  })
  text = text.replace(/\\\(([\s\S]+?)\\\)/g, (match, tex: string) => {
    const body = tex.trim()
    return body ? `$${body}$` : match
  })

  text = text.replace(/\u0000INLINE_CODE_(\d+)\u0000/g, (_, index) => inlineCodes[Number(index)] ?? '')
  text = text.replace(/\u0000CODE_BLOCK_(\d+)\u0000/g, (_, index) => blocks[Number(index)] ?? '')
  return text
}

const uploadError = ref('')

/**
 * Vditor hands the handler whatever the event carried: a `FileList` for a paste,
 * a `DataTransferItemList` for a drop. Neither is an array, and the drop entries
 * are `DataTransferItem`s that still need `getAsFile()`.
 */
type PastedItem = File | DataTransferItem
function toFiles(input: ArrayLike<PastedItem>): File[] {
  return Array.from(input)
    .map(item => (item instanceof File ? item : item.getAsFile()))
    .filter((f): f is File => !!f)
}

/** Save each pasted image, then insert a link to it at the cursor. */
async function handlePastedFiles(input: ArrayLike<PastedItem>) {
  const images = toFiles(input).filter(f => f.type.startsWith('image/'))
  if (!images.length || !props.imageUploader) return
  uploadError.value = ''
  for (const file of images) {
    try {
      const url = await props.imageUploader(file)
      if (!url) {
        uploadError.value = `不支持的图片格式：${file.type || '未知'}`
        continue
      }
      // insertValue emits `input`, so the note autosaves like any other edit.
      vd?.insertValue(`![](${url})\n`)
    } catch (e) {
      uploadError.value = String(e)
    }
  }
}

onMounted(async () => {
  // Vditor (JS + CSS) is heavyweight — load it when an editor actually mounts
  // instead of in every window's startup bundle.
  const [{ default: Vditor }] = await Promise.all([
    import('vditor'),
    import('vditor/dist/index.css'),
  ])
  await nextTick()
  if (!containerEl.value || destroyed) return

  const content = normalizeDisplayMathDelimiters(props.initialContent)

  vd = new Vditor(containerEl.value, {
    mode: 'ir',
    cdn: '/vditor',
    lang: 'zh_CN',
    value: content,
    minHeight: 200,
    toolbarConfig: { hide: true },
    preview: {
      math: {
        engine: 'KaTeX',
        inlineDigit: true,
      },
    },
    counter: { enable: false },
    resize: { enable: false },
    cache: { enable: false },
    // Vditor's own paste path, when neither `upload.url` nor `upload.handler` is
    // set, does `readAsDataURL` and drops the whole image into the document as
    // base64. A screenshot is megabytes of text on one line, re-parsed on every
    // keystroke and rewritten to disk on every autosave — that is what froze the
    // editor. Claiming `handler` takes that branch out of play entirely.
    upload: {
      handler: (files: File[]) => {
        void handlePastedFiles(files as unknown as ArrayLike<PastedItem>)
        return null
      },
    },
    after() {
      requestAnimationFrame(() => {
        if (content && vd?.getValue().trim() === '') {
          vd.setValue(content, true)
        }
      })
    },
    input(val) {
      emit('change', val)
    },
  })
})

onBeforeUnmount(() => {
  destroyed = true
  vd?.destroy()
  vd = null
  clearHighlights()
})

// ── Find bar ──────────────────────────────────────────────────────────────────
const findVisible = ref(false)
const findQuery = ref('')
const findInput = ref<HTMLInputElement | null>(null)
const matchCount = ref(0)
const matchIndex = ref(0) // 1-based

let allRanges: Range[] = []

// CSS Custom Highlight API — no DOM mutation, no focus side-effects
function cssHighlights(): HighlightRegistry | null {
  return (typeof CSS !== 'undefined' && 'highlights' in CSS)
    ? (CSS as unknown as { highlights: HighlightRegistry }).highlights
    : null
}

function applyHighlights() {
  const h = cssHighlights()
  if (!h) return
  h.delete('vditor-find-all')
  h.delete('vditor-find-current')
  if (allRanges.length === 0) return
  h.set('vditor-find-all', new Highlight(...allRanges))
  if (matchIndex.value > 0) {
    h.set('vditor-find-current', new Highlight(allRanges[matchIndex.value - 1]))
  }
}

function clearHighlights() {
  const h = cssHighlights()
  if (!h) return
  h.delete('vditor-find-all')
  h.delete('vditor-find-current')
}

function scrollToMatch(range: Range) {
  range.startContainer.parentElement?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
}

function getEditorRoot(): HTMLElement | null {
  return containerEl.value?.querySelector('.vditor-ir .vditor-reset') ?? null
}

function gatherRanges(query: string): Range[] {
  const root = getEditorRoot()
  if (!root || !query) return []

  const ranges: Range[] = []
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  const lower = query.toLowerCase()

  let node: Node | null
  while ((node = walker.nextNode())) {
    const text = (node.textContent ?? '').toLowerCase()
    let pos = 0
    while ((pos = text.indexOf(lower, pos)) !== -1) {
      const r = document.createRange()
      r.setStart(node, pos)
      r.setEnd(node, pos + query.length)
      ranges.push(r)
      pos += lower.length
    }
  }
  return ranges
}

function runSearch() {
  allRanges = gatherRanges(findQuery.value)
  matchCount.value = allRanges.length
  matchIndex.value = allRanges.length > 0 ? 1 : 0
  applyHighlights()
  if (allRanges.length > 0) scrollToMatch(allRanges[0])
}

function findNext() {
  if (allRanges.length === 0) return
  matchIndex.value = (matchIndex.value % allRanges.length) + 1
  applyHighlights()
  scrollToMatch(allRanges[matchIndex.value - 1])
}

function findPrev() {
  if (allRanges.length === 0) return
  matchIndex.value = ((matchIndex.value - 2 + allRanges.length) % allRanges.length) + 1
  applyHighlights()
  scrollToMatch(allRanges[matchIndex.value - 1])
}

function openFind() {
  findVisible.value = true
  nextTick(() => {
    findInput.value?.focus()
    findInput.value?.select()
    if (findQuery.value) runSearch()
  })
}

/**
 * Replace the editor's content from outside (a save that happened in another
 * window). `clearStack=true` suppresses the `input` event, so adopting a remote
 * change doesn't look like a local edit and bounce straight back out as a save.
 */
function setContent(markdown: string): boolean {
  // Vditor initialises asynchronously; report failure so the caller can remount
  // us with the new content instead of silently dropping it.
  if (!vd) return false
  if (vd.getValue() !== markdown) vd.setValue(markdown, true)
  return true
}

defineExpose({ setContent })

function closeFind() {
  clearHighlights()
  allRanges = []
  matchCount.value = 0
  matchIndex.value = 0
  findQuery.value = ''
  findVisible.value = false
  // Re-set content so Vditor replaces all Text nodes, invalidating any stale Highlight ranges.
  // Pass `clearStack=true` so this internal reset does NOT emit `input` — otherwise merely
  // opening/closing the find bar would mark the note dirty and trigger an autosave.
  if (vd) vd.setValue(vd.getValue(), true)
}

// Use @input on the input element instead of a watcher to avoid async flush races

function onWrapperKeydown(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && e.key === 'f') {
    e.preventDefault()
    e.stopPropagation()
    openFind()
  }
}

function onFindKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    if (e.shiftKey) findPrev()
    else findNext()
  } else if (e.key === 'Escape') {
    e.stopPropagation()
    closeFind()
  }
}
</script>

<template>
  <div class="vditor-wrapper" @keydown="onWrapperKeydown">
    <!-- Find bar -->
    <Transition name="find-bar">
      <div v-if="findVisible" class="find-bar">
        <input
          ref="findInput"
          v-model="findQuery"
          class="find-input"
          placeholder="搜索..."
          @input="runSearch"
          @keydown="onFindKeydown"
        />
        <span class="find-count">
          {{ findQuery ? (matchCount === 0 ? '无结果' : `${matchIndex} / ${matchCount}`) : '' }}
        </span>
        <button class="find-nav" :disabled="matchCount === 0" title="上一个 (Shift+Enter)" @mousedown.prevent @click="findPrev">
          <Icon icon="fluent:chevron-up-24-regular" width="10" height="10" />
        </button>
        <button class="find-nav" :disabled="matchCount === 0" title="下一个 (Enter)" @mousedown.prevent @click="findNext">
          <Icon icon="fluent:chevron-down-24-regular" width="10" height="10" />
        </button>
        <button class="find-close" title="关闭 (Esc)" @mousedown.prevent @click="closeFind">
          <Icon icon="fluent:dismiss-24-regular" width="10" height="10" />
        </button>
      </div>
    </Transition>

    <Transition name="find-bar">
      <div v-if="uploadError" class="upload-error" @click="uploadError = ''">{{ uploadError }}</div>
    </Transition>

    <div ref="containerEl" class="vditor-host" />
  </div>
</template>

<style scoped>
.vditor-wrapper {
  position: absolute;
  inset: 0;
  overflow: hidden;
}

.vditor-host {
  position: absolute;
  inset: 0;
  overflow-y: auto;
}

/* Find bar */
/* Image-paste failure, dismissed by clicking it. Sits below the find bar so the
   two never overlap when both are up. */
.upload-error {
  position: absolute;
  top: 44px;
  right: 12px;
  z-index: 100;
  max-width: 70%;
  background: var(--bg-secondary);
  border: 1px solid var(--danger, #dc2626);
  color: var(--danger, #dc2626);
  border-radius: var(--radius-sm);
  padding: 5px 9px;
  font-size: 12px;
  cursor: pointer;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
}

.find-bar {
  position: absolute;
  top: 8px;
  right: 12px;
  z-index: 100;
  display: flex;
  align-items: center;
  gap: 4px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  padding: 4px 6px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
}

.find-input {
  width: 160px;
  height: 22px;
  font-size: 12px;
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  padding: 0 6px;
  outline: none;
}
.find-input:focus {
  border-color: var(--accent);
}

.find-count {
  font-size: 11px;
  color: var(--text-tertiary);
  min-width: 52px;
  text-align: center;
  flex-shrink: 0;
}

.find-nav {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.find-nav:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.find-nav:disabled {
  opacity: 0.35;
  cursor: default;
}

.find-close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.find-close:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

/* Transition */
.find-bar-enter-active,
.find-bar-leave-active {
  transition: opacity 0.12s, transform 0.12s;
}
.find-bar-enter-from,
.find-bar-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>

<!-- CSS Custom Highlight API — global, not scoped -->
<style>
::highlight(vditor-find-all) {
  background-color: rgba(255, 200, 0, 0.4);
  color: inherit;
}
::highlight(vditor-find-current) {
  background-color: rgba(255, 140, 0, 0.7);
  color: #000;
}
</style>
