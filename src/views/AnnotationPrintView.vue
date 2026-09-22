<script setup lang="ts">
/**
 * Print preview for a highlights + notes export.
 *
 * PDF is produced through the system print dialog rather than written by the
 * app: a PDF authored with the bundled jsPDF has no CJK font and renders Chinese
 * as mojibake (the existing 导出文献列表 export has exactly that flaw), whereas
 * the webview already has the system's fonts. So this window lays the export out
 * as a document, and 「导出 PDF」 hands it to the OS, where "Save as PDF" lives.
 *
 * `print_annotation_window` goes through Tauri's `WebviewWindow::print` rather
 * than `window.print()`, because WKWebView does not implement the JS call — the
 * button would silently do nothing on macOS.
 */
import { ref, computed, onMounted } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import MarkdownBody from '../components/MarkdownBody.vue'
import type { ExportedPaper } from '../types'

const { t } = useI18n()

const loading = ref(true)
const error = ref('')
const papers = ref<ExportedPaper[]>([])
/** Mirrors the modal's granularity choice: one paper per printed page. */
const pageBreaks = ref(true)
const exportedAt = ref('')

const totalHighlights = computed(() =>
  papers.value.reduce((n, p) => n + p.highlights.length, 0)
)

function citation(p: ExportedPaper): string {
  const bits: string[] = []
  if (p.authors?.length) {
    bits.push(p.authors.length > 4 ? `${p.authors.slice(0, 3).join(', ')} 等` : p.authors.join(', '))
  }
  if (p.venue) bits.push(p.venue)
  if (p.year) bits.push(String(p.year))
  return bits.join(' · ')
}

function identifiers(p: ExportedPaper): string {
  const bits: string[] = []
  if (p.doi) bits.push(`DOI: ${p.doi}`)
  if (p.arxivId) bits.push(`arXiv: ${p.arxivId}`)
  if (p.tags?.length) bits.push(`标签: ${p.tags.join('、')}`)
  return bits.join(' · ')
}

/** Notes worth printing — an empty note is a heading with nothing under it. */
function filledNotes(p: ExportedPaper) {
  return (p.notes ?? []).filter(n => n.content.trim())
}

function isEmpty(p: ExportedPaper) {
  return p.highlights.length === 0 && filledNotes(p).length === 0
}

/** Group a paper's highlights by page, preserving the backend's ordering. */
function byPage(p: ExportedPaper) {
  const groups: { page: number; items: ExportedPaper['highlights'] }[] = []
  for (const h of p.highlights) {
    const last = groups[groups.length - 1]
    if (last && last.page === h.page) last.items.push(h)
    else groups.push({ page: h.page, items: [h] })
  }
  return groups
}

async function print() {
  try {
    await invoke('print_annotation_window')
  } catch (e) {
    error.value = String(e)
  }
}

onMounted(async () => {
  // Handed over through localStorage, the same way the paper-ai window receives
  // its slug: a window cannot take arguments, and a round trip through the
  // backend just to pass three values would be more moving parts than this.
  let slugs: string[] = []
  try {
    const raw = localStorage.getItem('argus:annotation-print')
    const parsed = raw ? JSON.parse(raw) : null
    slugs = Array.isArray(parsed?.slugs) ? parsed.slugs : []
    pageBreaks.value = parsed?.pageBreaks !== false
  } catch { /* malformed handoff — falls through to the empty state */ }

  exportedAt.value = new Date().toLocaleString()
  if (!slugs.length) {
    loading.value = false
    return
  }
  try {
    papers.value = await invoke<ExportedPaper[]>('collect_annotations', { slugs })
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <div class="ap-root">
    <!-- Toolbar: excluded from the printed page by @media print below. -->
    <div class="ap-toolbar" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <div class="ap-toolbar-text" data-tauri-drag-region>
        <strong data-tauri-drag-region>{{ t('annotationExport.previewTitle') }}</strong>
        <span v-if="!loading" data-tauri-drag-region>
          {{ t('annotationExport.summary', { papers: papers.length, highlights: totalHighlights }) }}
        </span>
      </div>
      <button class="ap-print-btn" :disabled="loading || !papers.length" @click="print">
        <Icon icon="fluent:print-24-regular" width="15" height="15" />
        {{ t('annotationExport.printPdf') }}
      </button>
    </div>

    <div v-if="error" class="ap-error">{{ error }}</div>

    <div class="ap-sheet">
      <div v-if="loading" class="ap-empty">{{ t('annotationExport.loading') }}</div>
      <div v-else-if="!papers.length" class="ap-empty">{{ t('annotationExport.nothing') }}</div>

      <template v-else>
        <header class="ap-head">
          <h1>{{ t('annotationExport.docTitle') }}</h1>
          <p class="ap-meta">
            {{ t('annotationExport.summaryLine', {
              at: exportedAt, papers: papers.length, highlights: totalHighlights,
            }) }}
          </p>
        </header>

        <section
          v-for="(p, i) in papers"
          :key="p.slug"
          class="ap-paper"
          :class="{ 'ap-break': pageBreaks && i > 0 }"
        >
          <h2>{{ p.title }}</h2>
          <p v-if="citation(p)" class="ap-cite">{{ citation(p) }}</p>
          <p v-if="identifiers(p)" class="ap-ids">{{ identifiers(p) }}</p>

          <p v-if="isEmpty(p)" class="ap-none">{{ t('annotationExport.paperEmpty') }}</p>

          <template v-if="p.highlights.length">
            <h3>{{ t('annotationExport.highlights', { n: p.highlights.length }) }}</h3>
            <div v-for="g in byPage(p)" :key="g.page" class="ap-page-group">
              <div class="ap-page-label">{{ t('annotationExport.page', { n: g.page }) }}</div>
              <div v-for="(h, hi) in g.items" :key="hi" class="ap-hl">
                <!-- The user's own highlight colour, as a left rule: it survives
                     printing where a background wash usually does not. -->
                <blockquote class="ap-quote" :style="{ borderLeftColor: h.color }">{{ h.text }}</blockquote>
                <p v-if="h.note" class="ap-hl-note">{{ h.note }}</p>
              </div>
            </div>
          </template>

          <template v-if="filledNotes(p).length">
            <h3>{{ t('annotationExport.notes') }}</h3>
            <div v-for="n in filledNotes(p)" :key="n.title" class="ap-note">
              <h4 class="ap-note-title">{{ n.title }}</h4>
              <MarkdownBody :content="n.content" />
            </div>
          </template>
        </section>
      </template>
    </div>
  </div>
</template>

<style scoped>
.ap-root {
  height: 100vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-primary, #f2f3f5);
  color: var(--text-primary, #1b1b1f);
  overflow: hidden;
}

.ap-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 52px;
  padding: 0 14px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border-color, #e4e6eb);
  background: var(--bg-secondary, #fff);
}
.ap-toolbar .tl-space { width: 82px; flex-shrink: 0; }
.ap-toolbar-text { display: flex; flex-direction: column; line-height: 1.3; flex: 1; min-width: 0; }
.ap-toolbar-text strong { font-size: 13px; }
.ap-toolbar-text span { font-size: 11.5px; color: var(--text-secondary, #8a8f98); }
.ap-print-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 7px 14px;
  border: none;
  border-radius: 7px;
  background: var(--accent, #3b82f6);
  color: #fff;
  font: inherit;
  cursor: pointer;
  flex-shrink: 0;
}
.ap-print-btn:disabled { opacity: 0.45; cursor: not-allowed; }

.ap-error {
  margin: 10px 14px 0;
  padding: 9px 11px;
  border-radius: 7px;
  background: rgba(220, 70, 70, 0.1);
  color: #c0392b;
  font-size: 12.5px;
}

/* The printable sheet. A fixed max-width so the on-screen preview matches the
   proportions of the printed page rather than stretching to the window. */
.ap-sheet {
  flex: 1;
  overflow-y: auto;
  padding: 26px 0 60px;
  margin: 0 auto;
  width: 100%;
  max-width: 760px;
  padding-left: 30px;
  padding-right: 30px;
  box-sizing: border-box;
  line-height: 1.7;
  font-size: 14px;
}
.ap-empty { text-align: center; color: var(--text-secondary, #8a8f98); padding: 60px 0; }

.ap-head h1 { font-size: 22px; margin-bottom: 6px; }
.ap-meta { color: var(--text-secondary, #8a8f98); font-size: 12.5px; margin-bottom: 8px; }

.ap-paper { padding-top: 22px; }
.ap-paper h2 { font-size: 18px; margin-bottom: 6px; line-height: 1.4; }
.ap-paper h3 { font-size: 14.5px; margin: 20px 0 8px; color: var(--text-secondary, #6b7280); }
.ap-paper h4 { font-size: 13.5px; margin: 14px 0 6px; }
.ap-cite { font-style: italic; color: var(--text-secondary, #6b7280); font-size: 13px; }
.ap-ids { color: var(--text-secondary, #8a8f98); font-size: 12px; margin-top: 2px; }
.ap-none { color: var(--text-secondary, #8a8f98); font-style: italic; margin-top: 10px; }

.ap-page-group { margin-bottom: 12px; }
.ap-page-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary, #8a8f98);
  margin: 12px 0 6px;
}
.ap-hl { margin-bottom: 10px; }
.ap-quote {
  margin: 0;
  padding: 2px 0 2px 12px;
  border-left: 3px solid #ffd400;
  white-space: pre-wrap;
  word-break: break-word;
}
.ap-hl-note {
  margin: 5px 0 0 12px;
  font-size: 13px;
  color: var(--text-secondary, #4b5563);
}
.ap-note { margin-bottom: 14px; }
/* The note's own title. Needs its own class because the `:deep` rules below
   match any `h4` inside `.ap-note` — including this one — and would shrink the
   title below the body headings it is supposed to outrank. */
.ap-note > .ap-note-title { font-size: 13.5px; margin: 14px 0 6px; }

/* A note is authored as a standalone document, so it often starts at `# `. Left
   alone, that `h1` renders at 28px and outranks the paper title above it,
   inverting the outline. The Markdown export solves this by demoting the
   headings; here the content is handed to MarkdownBody verbatim, so the cap is
   done in CSS instead — same intent, and it leaves the user's text untouched. */
.ap-note :deep(h1) { font-size: 13px; }
.ap-note :deep(h2) { font-size: 12.5px; }
.ap-note :deep(h3),
.ap-note :deep(h4),
.ap-note :deep(h5),
.ap-note :deep(h6) { font-size: 12px; }
.ap-note :deep(h1),
.ap-note :deep(h2),
.ap-note :deep(h3),
.ap-note :deep(h4),
.ap-note :deep(h5),
.ap-note :deep(h6) {
  font-weight: 600;
  margin: 12px 0 5px;
  line-height: 1.5;
}
.ap-note :deep(p) { margin: 0 0 8px; }
.ap-note :deep(ul),
.ap-note :deep(ol) { margin: 0 0 8px; padding-left: 20px; }
.ap-note :deep(pre) {
  background: var(--bg-primary, #f4f5f7);
  padding: 8px 10px;
  border-radius: 6px;
  overflow-x: auto;
  font-size: 12px;
}
.ap-note :deep(img) { max-width: 100%; }

/* Print: drop the chrome, unclip the scroller, and let the sheet use the page. */
@media print {
  .ap-toolbar,
  .ap-error { display: none !important; }
  .ap-root { height: auto; overflow: visible; background: #fff; }
  .ap-sheet {
    overflow: visible;
    max-width: none;
    padding: 0;
    font-size: 11pt;
  }
  /* One paper per sheet when the user asked for separate documents. */
  .ap-break { break-before: page; page-break-before: always; }
  /* Never strand a heading or split a single highlight across two sheets. */
  .ap-paper h2,
  .ap-paper h3,
  .ap-paper h4,
  .ap-page-label { break-after: avoid; page-break-after: avoid; }
  .ap-hl,
  .ap-quote { break-inside: avoid; page-break-inside: avoid; }
}
</style>
