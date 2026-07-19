<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { RecycleScroller } from 'vue-virtual-scroller'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import { titleInitialCaps } from '../utils/text'
import { noteBadgeStyle } from '../utils/noteBadges'
import { copyText } from '../utils/clipboard'
import {
  lists as writingLists,
  activeListId,
  loadLists as loadWritingLists,
  listById,
  addPapersToList,
  removePaperFromList,
} from '../stores/writing'
import type { PaperIndexEntry, PaperMeta } from '../types'

const { t } = useI18n()
const library = useLibraryStore()
const reader = useReaderStore()
const selection = useSelectionStore()

const emit = defineEmits<{ 'select-paper': [slug: string] }>()

// The list currently being viewed (null = all papers).
const activeList = computed(() => listById(activeListId.value))
onMounted(() => { void loadWritingLists() })

// ── Filter / search ───────────────────────────────────────────────────────────
const ONLY_BIBTEX_KEY = 'argus:writing:only-bibtex'
const query = ref('')
const onlyBibtex = ref(localStorage.getItem(ONLY_BIBTEX_KEY) === '1')
watch(onlyBibtex, (v) => localStorage.setItem(ONLY_BIBTEX_KEY, v ? '1' : '0'))

// ── Sort (lightweight, click the year / cite / title headers) ─────────────────
type SortField = 'added' | 'year' | 'cite' | 'title'
const sortField = ref<SortField>('added')
const sortDir = ref<'asc' | 'desc'>('desc')

function toggleSort(field: SortField) {
  if (sortField.value === field) {
    sortDir.value = sortDir.value === 'asc' ? 'desc' : 'asc'
  } else {
    sortField.value = field
    sortDir.value = field === 'title' ? 'asc' : 'desc'
  }
}

const filtered = computed<PaperIndexEntry[]>(() => {
  const q = query.value.trim().toLowerCase()
  let list = library.papers
  // Scope to the selected writing list (membership by paper id).
  const scoped = activeList.value
  if (scoped) {
    const ids = new Set(scoped.paper_ids)
    list = list.filter(p => ids.has(p.id))
  }
  if (onlyBibtex.value) list = list.filter(p => p.has_bibtex !== false)
  // Search matches title, authors and notes only (no full text).
  if (q) {
    list = list.filter(p =>
      p.title.toLowerCase().includes(q) ||
      p.authors.some(a => a.toLowerCase().includes(q)) ||
      notesFor(p).some(n => n.toLowerCase().includes(q))
    )
  }
  return list
})

const sorted = computed<PaperIndexEntry[]>(() => {
  const list = [...filtered.value]
  const dir = sortDir.value === 'asc' ? 1 : -1
  list.sort((a, b) => {
    switch (sortField.value) {
      case 'title': return a.title.localeCompare(b.title) * dir
      case 'year':  return ((a.year ?? 0) - (b.year ?? 0)) * dir
      case 'cite':  return ((a.cite_count ?? -1) - (b.cite_count ?? -1)) * dir
      default:      return a.added_at.localeCompare(b.added_at) * dir
    }
  })
  return list
})

function formatAuthors(authors: string[]): string {
  if (authors.length === 0) return '—'
  if (authors.length <= 2) return authors.join(', ')
  return authors[0] + ' et al.'
}

// ── Canvas note titles (same source as the library list's notes column) ───────
const noteTitles = ref<Record<string, string[]>>({})
let noteLoadSeq = 0

function notesFor(item: PaperIndexEntry): string[] {
  return noteTitles.value[item.slug] ?? []
}

// Load canvas note titles for every library paper so both the notes column and
// the notes search have data (search runs before the row is rendered).
watch(
  () => library.papers.map(p => p.slug).join('|'),
  async () => {
    const slugs = Array.from(new Set(library.papers.map(p => p.slug))).filter(Boolean)
    if (slugs.length === 0) { noteTitles.value = {}; return }
    const seq = ++noteLoadSeq
    try {
      const map = await invoke<Record<string, string[]>>('get_canvas_note_titles_map', { slugs })
      if (seq === noteLoadSeq) noteTitles.value = map
    } catch {
      if (seq === noteLoadSeq) noteTitles.value = {}
    }
  },
  { immediate: true },
)

// ── BibTeX / cite key (fetched lazily per paper, then cached) ──────────────────
// PaperIndexEntry only carries `has_bibtex`; the actual entry lives in PaperMeta.
const bibtexCache = ref<Record<string, string | null>>({})

async function ensureBibtex(slug: string): Promise<string | null> {
  if (slug in bibtexCache.value) return bibtexCache.value[slug]
  try {
    const meta = await invoke<PaperMeta>('get_paper_meta', { slug })
    const bib = meta.bibtex?.trim() || null
    bibtexCache.value = { ...bibtexCache.value, [slug]: bib }
    return bib
  } catch {
    return null
  }
}

// Pull the citation key out of a BibTeX entry: `@article{smith2020, ...}` → smith2020.
function parseCiteKey(bibtex: string): string | null {
  const m = bibtex.match(/@\s*\w+\s*\{\s*([^,\s}]+)/)
  return m ? m[1] : null
}

// ── Copy actions ──────────────────────────────────────────────────────────────
const copied = ref<{ slug: string; kind: 'key' | 'bibtex' } | null>(null)
let copiedTimer: ReturnType<typeof setTimeout> | null = null

function flashCopied(slug: string, kind: 'key' | 'bibtex') {
  copied.value = { slug, kind }
  if (copiedTimer) clearTimeout(copiedTimer)
  copiedTimer = setTimeout(() => { copied.value = null }, 1400)
}

async function writeClipboard(text: string): Promise<boolean> {
  const ok = await copyText(text)
  if (!ok) showToast(t('writing.copyFailed'))
  return ok
}

async function copyKey(item: PaperIndexEntry) {
  if (item.has_bibtex === false) { showToast(t('writing.noBibtex')); return }
  const bib = await ensureBibtex(item.slug)
  if (!bib) { showToast(t('writing.noBibtex')); return }
  const key = parseCiteKey(bib)
  if (!key) { showToast(t('writing.noKey')); return }
  if (await writeClipboard(key)) flashCopied(item.slug, 'key')
}

async function copyBibtex(item: PaperIndexEntry) {
  if (item.has_bibtex === false) { showToast(t('writing.noBibtex')); return }
  const bib = await ensureBibtex(item.slug)
  if (!bib) { showToast(t('writing.noBibtex')); return }
  if (await writeClipboard(bib)) flashCopied(item.slug, 'bibtex')
}

// ── Selection + one-click .bib export ─────────────────────────────────────────
const selected = ref<Set<string>>(new Set())

function toggleSelect(slug: string) {
  const next = new Set(selected.value)
  if (next.has(slug)) next.delete(slug); else next.add(slug)
  selected.value = next
}

const allVisibleSelected = computed(() =>
  sorted.value.length > 0 && sorted.value.every(p => selected.value.has(p.slug))
)

function toggleSelectAll() {
  if (allVisibleSelected.value) {
    const next = new Set(selected.value)
    for (const p of sorted.value) next.delete(p.slug)
    selected.value = next
  } else {
    const next = new Set(selected.value)
    for (const p of sorted.value) next.add(p.slug)
    selected.value = next
  }
}

const exporting = ref(false)

async function exportBib() {
  if (exporting.value || selected.value.size === 0) return
  exporting.value = true
  try {
    const slugs = sorted.value.filter(p => selected.value.has(p.slug)).map(p => p.slug)
    const entries: string[] = []
    let missing = 0
    for (const slug of slugs) {
      const bib = await ensureBibtex(slug)
      if (bib) entries.push(bib.trim())
      else missing++
    }
    if (entries.length === 0) { showToast(t('writing.noBibtex')); return }

    const savePath = await dialogSave({
      title: t('writing.exportBib'),
      defaultPath: 'references.bib',
      filters: [{ name: 'BibTeX', extensions: ['bib'] }],
    })
    if (!savePath) return

    const text = entries.join('\n\n') + '\n'
    const bytes = Array.from(new TextEncoder().encode(text))
    await invoke('write_bytes_to_file', { path: savePath, bytes })
    showToast(
      missing > 0
        ? t('writing.exportedPartial', { n: entries.length, missing })
        : t('writing.exported', { n: entries.length }),
    )
  } catch (e) {
    showToast(String(e))
  } finally {
    exporting.value = false
  }
}

// ── Add / remove selected papers to a writing list ────────────────────────────
const showAddMenu = ref(false)

function selectedIds(): string[] {
  return library.papers.filter(p => selected.value.has(p.slug)).map(p => p.id)
}

async function addSelectedToList(listId: string) {
  showAddMenu.value = false
  const ids = selectedIds()
  if (ids.length === 0) return
  await addPapersToList(listId, ids)
  showToast(t('writing.addedToList', { n: ids.length, name: listById(listId)?.name ?? '' }))
  selected.value = new Set()
}

async function removeSelectedFromCurrentList() {
  const listId = activeListId.value
  if (!listId) return
  const ids = selectedIds()
  if (ids.length === 0) return
  for (const id of ids) await removePaperFromList(listId, id)
  showToast(t('writing.removedFromList', { n: ids.length }))
  selected.value = new Set()
}

function onDocMousedown(e: MouseEvent) {
  if (!showAddMenu.value) return
  if ((e.target as HTMLElement)?.closest('.w-add-wrap')) return
  showAddMenu.value = false
}
onMounted(() => document.addEventListener('mousedown', onDocMousedown))
onBeforeUnmount(() => document.removeEventListener('mousedown', onDocMousedown))

// ── Select a paper → show its notes/metadata in the right sidebar ─────────────
function onRowClick(item: PaperIndexEntry) {
  selection.selectPaper(item.slug)
  emit('select-paper', item.slug)
}

// ── Open the paper in the reader (click on the title) ─────────────────────────
function openPaper(item: PaperIndexEntry) {
  reader.openPaper(item.slug, item.title, item.file_type)
}

// ── Transient toast ───────────────────────────────────────────────────────────
const toast = ref<string | null>(null)
let toastTimer: ReturnType<typeof setTimeout> | null = null

function showToast(msg: string) {
  toast.value = msg
  if (toastTimer) clearTimeout(toastTimer)
  toastTimer = setTimeout(() => { toast.value = null }, 2600)
}

onBeforeUnmount(() => {
  if (copiedTimer) clearTimeout(copiedTimer)
  if (toastTimer) clearTimeout(toastTimer)
})
</script>

<template>
  <div class="writing-view">
    <!-- Toolbar: search · only-bibtex · count · export selected -->
    <div class="w-toolbar">
      <div class="w-search">
        <Icon icon="fluent:search-24-regular" width="14" height="14" />
        <input
          v-model="query"
          class="w-search-input"
          :placeholder="t('writing.searchPlaceholder')"
        />
        <button v-if="query" class="w-search-clear" @click="query = ''">
          <Icon icon="fluent:dismiss-24-regular" width="12" height="12" />
        </button>
      </div>

      <label class="w-toggle" :class="{ on: onlyBibtex }">
        <input type="checkbox" v-model="onlyBibtex" />
        <span>{{ t('writing.onlyWithBibtex') }}</span>
      </label>

      <span class="w-count">
        <span v-if="activeList" class="w-scope">{{ activeList.name }}</span>
        {{ sorted.length }}
      </span>

      <!-- Membership actions (only with a selection) -->
      <template v-if="selected.size > 0">
        <div class="w-add-wrap">
          <button class="w-list-btn" @click.stop="showAddMenu = !showAddMenu">
            <Icon icon="fluent:folder-24-regular" width="14" height="14" />
            <span>{{ t('writing.addToList') }}</span>
          </button>
          <div v-if="showAddMenu" class="w-add-menu" @click.stop>
            <div v-if="writingLists.length === 0" class="w-add-empty">{{ t('writing.noLists') }}</div>
            <button
              v-for="l in writingLists"
              :key="l.id"
              class="w-add-item"
              @click="addSelectedToList(l.id)"
            >
              <Icon icon="fluent:document-text-24-regular" width="13" height="13" />
              <span>{{ l.name }}</span>
            </button>
          </div>
        </div>
        <button
          v-if="activeListId"
          class="w-list-btn"
          @click="removeSelectedFromCurrentList"
        >
          <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
          <span>{{ t('writing.removeFromList') }}</span>
        </button>
      </template>

      <button
        class="w-export-btn"
        :disabled="selected.size === 0 || exporting"
        :title="t('writing.exportBib')"
        @click="exportBib"
      >
        <Icon v-if="exporting" icon="fluent:spinner-ios-20-filled" width="14" height="14" class="spin" />
        <Icon v-else icon="fluent:arrow-download-24-regular" width="14" height="14" />
        <span>{{ selected.size > 0 ? t('writing.exportSelected', { n: selected.size }) : t('writing.exportBib') }}</span>
      </button>
    </div>

    <!-- Column header -->
    <div class="w-header">
      <div class="w-cell w-check">
        <input
          type="checkbox"
          :checked="allVisibleSelected"
          :title="t('writing.selectAll')"
          @change="toggleSelectAll"
        />
      </div>
      <div class="w-cell w-hdr" :class="{ active: sortField === 'title' }" @click="toggleSort('title')">
        {{ t('list.title') }}<span v-if="sortField === 'title'" class="w-arrow">{{ sortDir === 'asc' ? '↑' : '↓' }}</span>
      </div>
      <div class="w-cell">{{ t('list.authors') }}</div>
      <div class="w-cell w-hdr w-year" :class="{ active: sortField === 'year' }" @click="toggleSort('year')">
        {{ t('list.year') }}<span v-if="sortField === 'year'" class="w-arrow">{{ sortDir === 'asc' ? '↑' : '↓' }}</span>
      </div>
      <div class="w-cell w-hdr w-cite" :class="{ active: sortField === 'cite' }" @click="toggleSort('cite')">
        {{ t('list.citeCount') }}<span v-if="sortField === 'cite'" class="w-arrow">{{ sortDir === 'asc' ? '↑' : '↓' }}</span>
      </div>
      <div class="w-cell">{{ t('list.notes') }}</div>
      <div class="w-cell w-actions-hdr">BibTeX</div>
    </div>

    <!-- Empty state -->
    <div v-if="sorted.length === 0" class="w-empty">
      <Icon icon="fluent:document-text-24-regular" width="32" height="32" />
      <p>{{ query || onlyBibtex ? t('writing.noMatch') : (activeList ? t('writing.emptyList') : t('list.noPapers')) }}</p>
    </div>

    <!-- Virtualized rows -->
    <div v-else class="w-body">
      <RecycleScroller
        class="w-scroller"
        :items="sorted"
        :item-size="40"
        key-field="slug"
      >
        <template #default="{ item }: { item: PaperIndexEntry }">
          <div
            class="w-row"
            :class="{ selected: selected.has(item.slug), current: selection.selectedSlug === item.slug }"
            @click="onRowClick(item)"
          >
            <div class="w-cell w-check" @click.stop>
              <input
                type="checkbox"
                :checked="selected.has(item.slug)"
                @change="toggleSelect(item.slug)"
              />
            </div>
            <div class="w-cell w-title" :title="titleInitialCaps(item.title)" @click.stop="openPaper(item)">
              {{ titleInitialCaps(item.title) }}
            </div>
            <div class="w-cell w-authors" :title="item.authors.join(', ')">{{ formatAuthors(item.authors) }}</div>
            <div class="w-cell w-year">{{ item.year ?? '—' }}</div>
            <div class="w-cell w-cite">{{ item.cite_count != null ? item.cite_count.toLocaleString() : '—' }}</div>
            <div class="w-cell w-notes" :title="notesFor(item).join(', ')">
              <span
                v-for="(title, index) in notesFor(item)"
                :key="title"
                class="w-note-badge"
                :style="noteBadgeStyle(title, index)"
              >{{ title }}</span>
            </div>
            <div class="w-cell w-actions" @click.stop>
              <button
                class="w-copy-btn"
                :class="{ done: copied?.slug === item.slug && copied?.kind === 'key' }"
                :disabled="item.has_bibtex === false"
                :title="t('writing.copyKey')"
                @click="copyKey(item)"
              >
                <Icon
                  :icon="copied?.slug === item.slug && copied?.kind === 'key' ? 'fluent:checkmark-24-regular' : 'fluent:document-24-regular'"
                  width="13" height="13"
                />
                <span>key</span>
              </button>
              <button
                class="w-copy-btn"
                :class="{ done: copied?.slug === item.slug && copied?.kind === 'bibtex' }"
                :disabled="item.has_bibtex === false"
                :title="t('writing.copyBibtex')"
                @click="copyBibtex(item)"
              >
                <Icon
                  :icon="copied?.slug === item.slug && copied?.kind === 'bibtex' ? 'fluent:checkmark-24-regular' : 'fluent:copy-24-regular'"
                  width="13" height="13"
                />
                <span>BibTeX</span>
              </button>
            </div>
          </div>
        </template>
      </RecycleScroller>
    </div>

    <Transition name="w-toast">
      <div v-if="toast" class="w-toast" @click="toast = null">{{ toast }}</div>
    </Transition>
  </div>
</template>

<style scoped>
.writing-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  background: var(--bg-primary);
  position: relative;
}

/* ── Shared grid: checkbox · year · title · authors · cite · notes · actions ── */
.w-header,
.w-row {
  display: grid;
  grid-template-columns: 36px minmax(220px, 2.2fr) 168px 56px 68px minmax(150px, 1.1fr) 208px;
  align-items: center;
  width: 100%;
  min-width: 100%;
}

/* ── Toolbar ─────────────────────────────────────────────────────────────────── */
.w-toolbar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 10px;
  height: var(--content-header-height);
  padding: 0 12px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
}

.w-search {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: 1;
  max-width: 360px;
  height: 28px;
  padding: 0 8px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-tertiary);
}
.w-search-input {
  flex: 1;
  min-width: 0;
  border: none;
  background: transparent;
  color: var(--text-primary);
  font-size: 13px;
}
.w-search-input:focus { outline: none; }
.w-search-clear {
  display: inline-flex;
  color: var(--text-tertiary);
}
.w-search-clear:hover { color: var(--text-primary); }

.w-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: pointer;
  user-select: none;
  white-space: nowrap;
}
.w-toggle input { cursor: pointer; }
.w-toggle.on { color: var(--accent); }

.w-count {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
  color: var(--text-tertiary);
  background: var(--bg-tertiary);
  padding: 1px 7px;
  border-radius: var(--radius-pill);
  white-space: nowrap;
  margin-left: auto;
}
.w-scope {
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--accent);
  font-weight: 600;
}

/* Membership actions */
.w-add-wrap { position: relative; }
.w-list-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 500;
  white-space: nowrap;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.w-list-btn:hover {
  background: var(--accent-light);
  border-color: var(--accent);
  color: var(--accent);
}
.w-add-menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  min-width: 176px;
  max-height: 320px;
  overflow-y: auto;
  padding: 4px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: var(--bg-primary);
  box-shadow: var(--shadow-md);
  z-index: 120;
}
.w-add-item {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  padding: 6px 8px;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: 12px;
  text-align: left;
}
.w-add-item:hover { background: var(--bg-hover); }
.w-add-item svg { flex-shrink: 0; color: var(--text-tertiary); }
.w-add-empty {
  padding: 8px;
  font-size: 12px;
  color: var(--text-tertiary);
  text-align: center;
}

.w-export-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 12px;
  border-radius: var(--radius-md);
  background: var(--accent);
  color: #fff;
  font-size: 12px;
  font-weight: 500;
  white-space: nowrap;
  transition: opacity 0.12s;
}
.w-export-btn:hover:not(:disabled) { opacity: 0.88; }
.w-export-btn:disabled { opacity: 0.4; cursor: not-allowed; }

/* ── Header row ──────────────────────────────────────────────────────────────── */
.w-header {
  flex-shrink: 0;
  height: 34px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
}
.w-header .w-cell {
  font-size: 11px;
  font-weight: 500;
  color: var(--text-tertiary);
}
.w-hdr {
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 3px;
}
.w-hdr:hover { color: var(--text-secondary); }
.w-hdr.active { color: var(--accent); }
.w-arrow { font-size: 10px; }

/* ── Body / rows ─────────────────────────────────────────────────────────────── */
.w-body {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}
.w-scroller {
  height: 100%;
  overflow-y: auto;
}
.w-row {
  height: 40px;
  border-bottom: 1px solid var(--border-subtle);
  transition: background 0.1s;
}
.w-row:hover { background: var(--bg-hover); }
.w-row.selected {
  background: var(--bg-selected);
}
.w-row.current {
  background: var(--bg-selected);
  box-shadow: inset 2.5px 0 0 var(--accent);
}

.w-cell {
  min-width: 0;
  padding: 0 8px;
  font-size: 12px;
  color: var(--text-tertiary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.w-check {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}
.w-check input { cursor: pointer; }
.w-year { font-variant-numeric: tabular-nums; }
.w-cite { font-variant-numeric: tabular-nums; }

.w-title {
  font-size: 13px;
  font-weight: 500;
  color: var(--text-primary);
  cursor: pointer;
}
.w-title:hover { color: var(--accent); text-decoration: underline; }

.w-authors { color: var(--text-secondary); }

.w-notes {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: nowrap;
}
.w-note-badge {
  display: inline-block;
  max-width: 120px;
  padding: 2px 7px;
  border-radius: 5px;
  border: 1px solid transparent;
  font-size: 10px;
  font-weight: 600;
  line-height: 1.25;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex-shrink: 0;
}

.w-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
}
.w-copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 24px;
  padding: 0 8px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 500;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.w-copy-btn:hover:not(:disabled) {
  background: var(--accent-light);
  border-color: var(--accent);
  color: var(--accent);
}
.w-copy-btn.done {
  background: color-mix(in srgb, #22c55e 14%, transparent);
  border-color: #22c55e;
  color: #16a34a;
}
.w-copy-btn:disabled { opacity: 0.4; cursor: not-allowed; }

/* ── Empty state ─────────────────────────────────────────────────────────────── */
.w-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-tertiary);
  padding: 32px;
}
.w-empty p { font-size: var(--font-size-md); color: var(--text-secondary); }

@keyframes spin { to { transform: rotate(360deg); } }
.spin { animation: spin 0.9s linear infinite; }

/* ── Toast ───────────────────────────────────────────────────────────────────── */
.w-toast {
  position: absolute;
  bottom: 16px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 100;
  padding: 8px 16px;
  border-radius: var(--radius-md);
  background: var(--bg-inverse, #1f2937);
  color: var(--text-inverse, #fff);
  font-size: 12px;
  box-shadow: var(--shadow-md);
  cursor: pointer;
  max-width: 80%;
}
.w-toast-enter-active, .w-toast-leave-active { transition: opacity 0.2s, transform 0.2s; }
.w-toast-enter-from, .w-toast-leave-to { opacity: 0; transform: translateX(-50%) translateY(8px); }
</style>
