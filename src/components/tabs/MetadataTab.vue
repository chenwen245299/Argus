<script setup lang="ts">
import { ref, watch, computed, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import type { PaperMeta } from '../../types'
import { useLibraryStore } from '../../stores/library'

const props = defineProps<{
  slug: string | null
  meta: PaperMeta | null
}>()

const emit = defineEmits<{
  saved: [meta: PaperMeta]
  slugChanged: [newSlug: string]
}>()

const { t } = useI18n()
const library = useLibraryStore()

// ── Source options ────────────────────────────────────────────────────────────
type ImportSource = 'file' | 'arxiv' | 'url'

const SOURCE_LABEL: Record<ImportSource, string> = { arxiv: 'ArXiv', file: '文件', url: '链接' }
const SOURCE_OPTIONS = [
  { value: 'file' as const,  label: '文件' },
  { value: 'arxiv' as const, label: 'ArXiv' },
  { value: 'url' as const,   label: '链接' },
]

function importSource(value?: string | null, arxivId?: string | null): ImportSource {
  if (value === 'arxiv' || value === 'url' || value === 'file') return value
  if (arxivId?.trim()) return 'arxiv'
  return 'file'
}

// ── Edit state ────────────────────────────────────────────────────────────────
const editing = ref(false)
const draft = ref<PaperMeta | null>(null)
const copiedKind = ref<'abstract' | 'fulltext' | 'bibtex' | null>(null)
const sourceEditing = ref(false)
const sourceDraft = ref<ImportSource>('file')
const sourceSaving = ref(false)

function startEdit() {
  if (!props.meta) return
  sourceEditing.value = false
  draft.value = JSON.parse(JSON.stringify(props.meta)) // deep clone
  draft.value!.import_source = importSource(draft.value!.import_source, draft.value!.arxiv_id)
  editing.value = true
}

function cancelEdit() {
  draft.value = null
  editing.value = false
}

async function saveEdit() {
  if (!props.slug || !draft.value) return
  saving.value = true
  try {
    draft.value.import_source = importSource(draft.value.import_source, draft.value.arxiv_id)
    await invoke('save_paper_meta', { slug: props.slug, meta: draft.value })
    emit('saved', { ...draft.value })
    editing.value = false
    draft.value = null
  } catch (e) {
    console.error('Failed to save meta:', e)
  } finally {
    saving.value = false
  }
}

// Reset when paper changes
watch(() => props.slug, () => {
  editing.value = false
  draft.value = null
  refetchOk.value = false
  renameOk.value = false
  copiedKind.value = null
  sourceEditing.value = false
  sourceDraft.value = 'file'
  bibtexEditing.value = false
  bibtexDraft.value = ''
  citeCountEditing.value = false
  citeCountDraft.value = undefined
  fulltextEditing.value = false
  fulltextDraft.value = ''
  fulltextError.value = ''
})

function startSourceEdit() {
  sourceDraft.value = importSource(props.meta?.import_source, props.meta?.arxiv_id)
  sourceEditing.value = true
}

function cancelSourceEdit() {
  sourceEditing.value = false
  sourceDraft.value = 'file'
}

async function saveSource() {
  if (!props.slug || !props.meta) return
  sourceSaving.value = true
  try {
    const updated: PaperMeta = { ...props.meta, import_source: sourceDraft.value }
    await invoke('save_paper_meta', { slug: props.slug, meta: updated })
    emit('saved', updated)
    sourceEditing.value = false
  } catch (e) {
    console.error('Failed to save source:', e)
  } finally {
    sourceSaving.value = false
  }
}

// ── BibTeX inline edit ────────────────────────────────────────────────────────
const bibtexEditing = ref(false)
const bibtexDraft = ref('')
const bibtexSaving = ref(false)

function startBibtexEdit() {
  bibtexDraft.value = props.meta?.bibtex ?? ''
  bibtexEditing.value = true
}

function cancelBibtexEdit() {
  bibtexEditing.value = false
  bibtexDraft.value = ''
}

async function saveBibtex() {
  if (!props.slug || !props.meta) return
  bibtexSaving.value = true
  try {
    const updated: PaperMeta = { ...props.meta, bibtex: bibtexDraft.value.trim() || undefined }
    await invoke('save_paper_meta', { slug: props.slug, meta: updated })
    emit('saved', updated)
    bibtexEditing.value = false
    bibtexDraft.value = ''
  } catch (e) {
    console.error('Failed to save bibtex:', e)
  } finally {
    bibtexSaving.value = false
  }
}

// ── Citation count inline edit ────────────────────────────────────────────────
const citeCountEditing = ref(false)
const citeCountDraft = ref<number | undefined>(undefined)
const citeCountSaving = ref(false)

function startCiteCountEdit() {
  citeCountDraft.value = props.meta?.cite_count
  citeCountEditing.value = true
}

function cancelCiteCountEdit() {
  citeCountEditing.value = false
  citeCountDraft.value = undefined
}

async function saveCiteCount() {
  if (!props.slug || !props.meta) return
  citeCountSaving.value = true
  try {
    const val = citeCountDraft.value
    const updated: PaperMeta = {
      ...props.meta,
      cite_count: (val != null && val >= 0) ? Math.floor(val) : undefined,
    }
    await invoke('save_paper_meta', { slug: props.slug, meta: updated })
    emit('saved', updated)
    citeCountEditing.value = false
    citeCountDraft.value = undefined
  } catch (e) {
    console.error('Failed to save cite_count:', e)
  } finally {
    citeCountSaving.value = false
  }
}

// ── Author / tag array helpers ────────────────────────────────────────────────
function addAuthor() {
  if (!draft.value) return
  draft.value.authors.push('')
}

function removeAuthor(i: number) {
  draft.value?.authors.splice(i, 1)
}

function removeTag(i: number) {
  draft.value?.tags.splice(i, 1)
}

// ── Tag autocomplete ──────────────────────────────────────────────────────────
const tagInput = ref('')
const tagInputFocused = ref(false)

const tagSuggestions = computed(() => {
  const q = tagInput.value.trim().toLowerCase()
  const existing = new Set(draft.value?.tags ?? [])
  return library.allTags.filter(t => !existing.has(t) && (q === '' || t.toLowerCase().includes(q)))
})

function addTagFromInput() {
  const val = tagInput.value.trim()
  if (!val || !draft.value) return
  if (!draft.value.tags.includes(val)) {
    draft.value.tags.push(val)
  }
  tagInput.value = ''
}

function addTagFromSuggestion(tag: string) {
  if (!draft.value || draft.value.tags.includes(tag)) return
  draft.value.tags.push(tag)
  tagInput.value = ''
}

function onTagInputBlur() {
  window.setTimeout(() => { tagInputFocused.value = false }, 150)
}

// ── Re-fetch & rename ─────────────────────────────────────────────────────────
const saving = ref(false)
const refetching = ref(false)
const renaming = ref(false)
const refetchOk = ref(false)
const renameOk = ref(false)

async function refetch() {
  if (!props.slug) return
  refetching.value = true
  refetchOk.value = false
  try {
    const updated = await invoke<PaperMeta>('fetch_metadata', { slug: props.slug })
    emit('saved', updated)
    refetchOk.value = true
    setTimeout(() => { refetchOk.value = false }, 2500)
  } catch (e) {
    console.error('Refetch failed:', e)
  } finally {
    refetching.value = false
  }
}

async function renameFolder() {
  if (!props.slug) return
  renaming.value = true
  renameOk.value = false
  try {
    const newSlug = await invoke<string>('rename_paper_folder', { slug: props.slug })
    renameOk.value = true
    emit('slugChanged', newSlug)
    setTimeout(() => { renameOk.value = false }, 2500)
  } catch (e) {
    console.error('Rename failed:', e)
  } finally {
    renaming.value = false
  }
}

function fmtDate(iso: string) {
  try { return new Date(iso).toLocaleDateString() } catch { return iso }
}

// ── Fulltext ──────────────────────────────────────────────────────────────────
const fulltext = ref('')
const fulltextLoading = ref(false)
const fulltextEditing = ref(false)
const fulltextDraft = ref('')
const fulltextSaving = ref(false)
const fulltextError = ref('')

async function loadFulltext(slug: string | null) {
  if (!slug) { fulltext.value = ''; return }
  fulltextLoading.value = true
  try {
    fulltext.value = await invoke<string>('get_fulltext', { slug })
  } catch {
    fulltext.value = ''
  } finally {
    fulltextLoading.value = false
  }
}

function startFulltextEdit() {
  fulltextDraft.value = fulltext.value
  fulltextError.value = ''
  fulltextEditing.value = true
}

function cancelFulltextEdit() {
  fulltextEditing.value = false
  fulltextDraft.value = ''
  fulltextError.value = ''
}

async function saveFulltext() {
  if (!props.slug) return
  fulltextSaving.value = true
  fulltextError.value = ''
  try {
    await invoke('save_fulltext', { slug: props.slug, text: fulltextDraft.value })
    fulltext.value = fulltextDraft.value
    fulltextEditing.value = false
    fulltextDraft.value = ''
    await library.refresh()
    window.dispatchEvent(new CustomEvent('argus-paper-fulltext-updated', {
      detail: { slug: props.slug },
    }))
  } catch (e) {
    fulltextError.value = String(e)
  } finally {
    fulltextSaving.value = false
  }
}

watch(() => props.slug, (slug) => loadFulltext(slug), { immediate: true })

function onFulltextUpdated(e: Event) {
  const slug = (e as CustomEvent<{ slug?: string }>).detail?.slug
  if (slug && slug === props.slug) loadFulltext(slug)
}

onMounted(() => { window.addEventListener('argus-paper-fulltext-updated', onFulltextUpdated) })
onBeforeUnmount(() => { window.removeEventListener('argus-paper-fulltext-updated', onFulltextUpdated) })

async function copyText(kind: 'abstract' | 'fulltext' | 'bibtex', text: string) {
  const val = text.trim()
  if (!val) return
  await navigator.clipboard.writeText(val).catch(() => {})
  copiedKind.value = kind
  window.setTimeout(() => {
    if (copiedKind.value === kind) copiedKind.value = null
  }, 1600)
}

// ── Abstract (AI-generated) ───────────────────────────────────────────────────
const abstractText = computed(() => props.meta?.abstract ?? '')
const abstractExtracting = ref(false)
const abstractError = ref('')

async function extractAbstract() {
  if (!props.slug) return
  abstractExtracting.value = true
  abstractError.value = ''
  try {
    const updated = await invoke<PaperMeta>(
      'extract_abstract_ai',
      { slug: props.slug, providerId: null, modelId: null },
    )
    emit('saved', updated)
  } catch (e) {
    abstractError.value = String(e)
  } finally {
    abstractExtracting.value = false
  }
}
</script>

<template>
  <div class="meta-tab">
    <!-- No paper selected -->
    <div v-if="!meta" class="empty">{{ t('meta.noSelection') }}</div>

    <template v-else>
      <!-- Action buttons (always visible) -->
      <div class="action-bar">
        <template v-if="!editing">
          <button class="act-btn primary" @click="startEdit">{{ t('metaEdit.edit') }}</button>
          <button class="act-btn" :disabled="refetching" @click="refetch">
            <template v-if="refetching">{{ t('metaEdit.refetching') }}</template>
            <template v-else-if="refetchOk">✓ {{ t('metaEdit.refetchOk') }}</template>
            <template v-else>{{ t('metaEdit.refetch') }}</template>
          </button>
          <button class="act-btn" :disabled="renaming" @click="renameFolder">
            <template v-if="renaming">{{ t('metaEdit.renaming') }}</template>
            <template v-else-if="renameOk">✓ {{ t('metaEdit.renameOk') }}</template>
            <template v-else>{{ t('metaEdit.renameFolder') }}</template>
          </button>
        </template>

        <template v-else>
          <button class="act-btn primary" :disabled="saving" @click="saveEdit">{{ t('metaEdit.save') }}</button>
          <button class="act-btn" @click="cancelEdit">{{ t('metaEdit.cancel') }}</button>
        </template>
      </div>

      <!-- ── READ VIEW ─────────────────────────────────── -->
      <template v-if="!editing">
        <div class="field">
          <div class="label">{{ t('meta.title') }}</div>
          <div class="value">{{ meta.title }}</div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.authors') }}</div>
          <div class="value">{{ meta.authors.join('; ') || '—' }}</div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.year') }}</div>
          <div class="value">{{ meta.year ?? '—' }}</div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.venue') }}</div>
          <div class="value">{{ meta.venue || '—' }}</div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.doi') }}</div>
          <div class="value mono">{{ meta.doi || '—' }}</div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.arxivId') }}</div>
          <div class="value mono">{{ meta.arxiv_id || '—' }}</div>
        </div>

        <!-- Citation count -->
        <div class="field">
          <div class="label cite-count-label-row">
            <span>{{ t('meta.citeCount') }}</span>
            <button class="copy-section-btn" @click="startCiteCountEdit">
              {{ meta.cite_count != null ? '编辑' : '导入' }}
            </button>
          </div>
          <template v-if="citeCountEditing">
            <div class="cite-count-edit-row">
              <input
                v-model.number="citeCountDraft"
                class="input cite-count-input"
                type="number"
                min="0"
                placeholder="0"
                autofocus
              />
              <button class="act-btn primary" :disabled="citeCountSaving" @click="saveCiteCount">
                {{ citeCountSaving ? '保存中…' : '保存' }}
              </button>
              <button class="act-btn" @click="cancelCiteCountEdit">取消</button>
            </div>
          </template>
          <template v-else>
            <div v-if="meta.cite_count == null" class="fulltext-placeholder muted">暂无引用量，点击导入添加</div>
            <div v-else class="value cite-count-val">{{ meta.cite_count.toLocaleString() }}</div>
          </template>
        </div>
        <div class="field tags-field">
          <div class="label">{{ t('meta.tags') }}</div>
          <div class="value tags">
            <template v-if="meta.tags.length > 0">
              <span v-for="tag in meta.tags" :key="tag" class="tag">{{ tag }}</span>
            </template>
            <span v-else class="muted">{{ t('meta.none') }}</span>
          </div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.readingStatus') }}</div>
          <div class="value">
            <span class="status-badge" :class="'status-' + meta.reading_status">
              {{ t('readingStatus.' + meta.reading_status) }}
            </span>
          </div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.added') }}</div>
          <div class="value muted">{{ fmtDate(meta.added_at) }}</div>
        </div>
        <div v-if="meta.original_filename" class="field">
          <div class="label">{{ t('meta.file') }}</div>
          <div class="value mono small">{{ meta.original_filename }}</div>
        </div>

        <!-- BibTeX -->
        <div class="field bibtex-field">
          <div class="label bibtex-label-row">
            <span>BibTeX</span>
            <div class="section-actions">
              <button class="copy-section-btn" @click="startBibtexEdit">
                {{ meta.bibtex ? '编辑' : '导入' }}
              </button>
              <button
                class="copy-section-btn"
                :class="{ done: copiedKind === 'bibtex' }"
                :disabled="!meta.bibtex"
                title="复制 BibTeX"
                @click="copyText('bibtex', meta.bibtex ?? '')"
              >
                <svg v-if="copiedKind === 'bibtex'" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <polyline points="20 6 9 17 4 12"/>
                </svg>
                <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                </svg>
                {{ copiedKind === 'bibtex' ? '已复制' : '复制' }}
              </button>
            </div>
          </div>
          <!-- Inline editor -->
          <template v-if="bibtexEditing">
            <textarea
              v-model="bibtexDraft"
              class="bibtex-textarea"
              placeholder="@article{key,&#10;  author = {Author Name},&#10;  title  = {Title},&#10;  year   = {2024},&#10;  ...&#10;}"
              autofocus
            />
            <div class="bibtex-edit-actions">
              <button class="act-btn primary" :disabled="bibtexSaving" @click="saveBibtex">
                {{ bibtexSaving ? '保存中…' : '保存' }}
              </button>
              <button class="act-btn" @click="cancelBibtexEdit">取消</button>
            </div>
          </template>
          <template v-else>
            <div v-if="!meta.bibtex" class="fulltext-placeholder muted">暂无 BibTeX，点击导入添加</div>
            <pre v-else class="bibtex-block">{{ meta.bibtex }}</pre>
          </template>
        </div>

        <!-- Abstract (AI-generated) -->
        <div class="field abstract-field">
          <div class="label abstract-label-row">
            <span>{{ t('meta.abstract') }}</span>
            <div class="section-actions">
              <button class="abstract-btn" :disabled="abstractExtracting" @click="extractAbstract">
                <svg v-if="abstractExtracting" width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="spin-xs"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>
                {{ abstractExtracting ? t('meta.abstractExtracting') : abstractText ? t('meta.abstractReExtract') : t('meta.abstractExtract') }}
              </button>
              <button
                class="copy-section-btn"
                :class="{ done: copiedKind === 'abstract' }"
                :disabled="!abstractText"
                title="复制摘要"
                @click="copyText('abstract', abstractText)"
              >
                <svg v-if="copiedKind === 'abstract'" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <polyline points="20 6 9 17 4 12"/>
                </svg>
                <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                </svg>
                {{ copiedKind === 'abstract' ? '已复制' : '复制' }}
              </button>
            </div>
          </div>
          <div v-if="abstractError" class="abstract-error">{{ abstractError }}</div>
          <div v-else-if="!abstractText" class="fulltext-placeholder muted">{{ t('meta.abstractNone') }}</div>
          <div v-else class="abstract-text">{{ abstractText }}</div>
        </div>

        <!-- Full text -->
        <div class="field fulltext-field">
          <div class="label fulltext-label-row">
            <span>{{ t('meta.fulltext') }}</span>
            <div class="section-actions">
              <span v-if="fulltext" class="fulltext-chars">{{ t('meta.fulltextWords', { n: fulltext.trim().split(/\s+/).length.toLocaleString() }) }}</span>
              <button
                class="copy-section-btn"
                :disabled="fulltextLoading || fulltextSaving"
                @click="startFulltextEdit"
              >
                {{ t('meta.fulltextEdit') }}
              </button>
              <button
                class="copy-section-btn"
                :class="{ done: copiedKind === 'fulltext' }"
                :disabled="!fulltext"
                title="复制全文"
                @click="copyText('fulltext', fulltext)"
              >
                <svg v-if="copiedKind === 'fulltext'" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                  <polyline points="20 6 9 17 4 12"/>
                </svg>
                <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
                </svg>
                {{ copiedKind === 'fulltext' ? '已复制' : '复制' }}
              </button>
            </div>
          </div>
          <template v-if="fulltextEditing">
            <textarea
              v-model="fulltextDraft"
              class="fulltext-box fulltext-editor"
              :placeholder="t('meta.fulltextPlaceholder')"
              autofocus
            />
            <div v-if="fulltextError" class="fulltext-error">{{ fulltextError }}</div>
            <div class="fulltext-edit-actions">
              <button class="act-btn primary" :disabled="fulltextSaving" @click="saveFulltext">
                {{ fulltextSaving ? t('meta.fulltextSaving') : t('meta.fulltextSave') }}
              </button>
              <button class="act-btn" :disabled="fulltextSaving" @click="cancelFulltextEdit">
                {{ t('meta.fulltextCancel') }}
              </button>
            </div>
          </template>
          <template v-else>
            <div v-if="fulltextLoading" class="fulltext-placeholder muted">…</div>
            <div v-else-if="!fulltext" class="fulltext-placeholder muted">{{ t('meta.fulltextNone') }}</div>
            <textarea v-else class="fulltext-box" readonly :value="fulltext" />
          </template>
        </div>

        <!-- Source (来源) -->
        <div class="field source-field">
          <div class="label source-label-row">
            <span>{{ t('list.source') }}</span>
            <button
              v-if="!sourceEditing"
              class="copy-section-btn"
              @click="startSourceEdit"
            >
              {{ t('metaEdit.edit') }}
            </button>
          </div>
          <template v-if="sourceEditing">
            <div class="source-btns">
              <button
                v-for="s in SOURCE_OPTIONS"
                :key="s.value"
                class="source-btn"
                :class="{ active: sourceDraft === s.value }"
                @click="sourceDraft = s.value"
              >{{ s.label }}</button>
            </div>
            <div class="source-edit-actions">
              <button class="act-btn primary" :disabled="sourceSaving" @click="saveSource">
                {{ sourceSaving ? t('meta.fulltextSaving') : t('metaEdit.save') }}
              </button>
              <button class="act-btn" :disabled="sourceSaving" @click="cancelSourceEdit">
                {{ t('metaEdit.cancel') }}
              </button>
            </div>
          </template>
          <div v-else class="value source-val">
            <span class="src-chip" :class="'src-' + importSource(meta.import_source, meta.arxiv_id)">
              {{ SOURCE_LABEL[importSource(meta.import_source, meta.arxiv_id)] }}
            </span>
          </div>
        </div>
      </template>

      <!-- ── EDIT VIEW ─────────────────────────────────── -->
      <template v-else-if="draft">
        <div class="field">
          <div class="label">{{ t('meta.title') }}</div>
          <input v-model="draft.title" class="input" type="text" />
        </div>

        <div class="field">
          <div class="label">{{ t('meta.authors') }}</div>
          <div v-for="(_, i) in draft.authors" :key="i" class="array-row">
            <input
              v-model="draft.authors[i]"
              class="input flex1"
              type="text"
              :placeholder="t('metaEdit.authorPlaceholder')"
            />
            <button class="rm-btn" @click="removeAuthor(i)">×</button>
          </div>
          <button class="add-btn" @click="addAuthor">+ {{ t('metaEdit.addAuthor') }}</button>
        </div>

        <div class="field">
          <div class="label">{{ t('meta.year') }}</div>
          <input
            v-model.number="draft.year"
            class="input"
            type="number"
            min="1000"
            max="2100"
            placeholder="YYYY"
          />
        </div>

        <div class="field">
          <div class="label">{{ t('meta.venue') }}</div>
          <input v-model="draft.venue" class="input" type="text" />
        </div>

        <div class="field">
          <div class="label">{{ t('meta.citeCount') }}</div>
          <input
            v-model.number="draft.cite_count"
            class="input"
            type="number"
            min="0"
            placeholder="0"
          />
        </div>

        <div class="field">
          <div class="label">{{ t('meta.doi') }}</div>
          <input v-model="draft.doi" class="input mono" type="text" placeholder="10.xxxx/…" />
        </div>

        <div class="field">
          <div class="label">{{ t('meta.arxivId') }}</div>
          <input v-model="draft.arxiv_id" class="input mono" type="text" placeholder="YYMM.NNNNN" />
        </div>

        <div class="field tags-field">
          <div class="label">{{ t('meta.tags') }}</div>
          <!-- Current tags as removable chips -->
          <div v-if="draft.tags.length" class="tag-chips">
            <span v-for="(tag, i) in draft.tags" :key="tag" class="tag-chip">
              {{ tag }}
              <button class="tag-chip-rm" @click="removeTag(i)">×</button>
            </span>
          </div>
          <!-- Autocomplete input -->
          <div class="tag-input-wrap">
            <input
              v-model="tagInput"
              class="input"
              type="text"
              :placeholder="t('metaEdit.tagPlaceholder')"
              @focus="tagInputFocused = true"
              @blur="onTagInputBlur"
              @keydown.enter.prevent="addTagFromInput"
              @keydown.comma.prevent="addTagFromInput"
            />
            <!-- Suggestions dropdown -->
            <div v-if="tagInputFocused && tagSuggestions.length" class="tag-suggestions">
              <button
                v-for="s in tagSuggestions.slice(0, 8)"
                :key="s"
                class="tag-suggestion"
                @mousedown.prevent="addTagFromSuggestion(s)"
              >{{ s }}</button>
            </div>
          </div>
        </div>
        <div class="field">
          <div class="label">{{ t('meta.readingStatus') }}</div>
          <div class="status-btns">
            <button
              v-for="s in ['unread', 'reading', 'read']"
              :key="s"
              class="status-btn"
              :class="{ active: draft.reading_status === s }"
              @click="draft.reading_status = s"
            >{{ t('readingStatus.' + s) }}</button>
          </div>
        </div>

        <div class="field">
          <div class="label">{{ t('list.source') }}</div>
          <div class="source-btns">
            <button
              v-for="s in SOURCE_OPTIONS"
              :key="s.value"
              class="source-btn"
              :class="{ active: importSource(draft.import_source) === s.value }"
              @click="draft.import_source = s.value"
            >{{ s.label }}</button>
          </div>
        </div>

        <!-- Read-only fields in edit mode -->
        <div v-if="meta.added_at" class="field">
          <div class="label">{{ t('meta.added') }}</div>
          <div class="value muted">{{ fmtDate(meta.added_at) }}</div>
        </div>
        <div v-if="meta.original_filename" class="field">
          <div class="label">{{ t('meta.file') }}</div>
          <div class="value mono small">{{ meta.original_filename }}</div>
        </div>
      </template>
    </template>
  </div>
</template>

<style scoped>
.meta-tab {
  padding: 10px 12px;
  overflow-y: auto;
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: 0;
}

.empty { color: var(--text-tertiary); font-size: var(--font-size-sm); padding: 16px; text-align: center; }

.action-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  padding-bottom: 10px;
  margin-bottom: 4px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.act-btn {
  font-size: var(--font-size-xs);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
  color: var(--text-secondary);
  transition: background 0.1s, color 0.1s;
  cursor: pointer;
  white-space: nowrap;
}
.act-btn:hover:not(:disabled) { background: var(--bg-tertiary); color: var(--text-primary); }
.act-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.act-btn.primary {
  background: var(--accent);
  color: #fff;
  border-color: transparent;
}
.act-btn.primary:hover:not(:disabled) { background: var(--accent-hover); }

.field { margin-bottom: 12px; }

.tags-field {
  padding: 6px 0 9px;
  border-top: 1px solid var(--border-default);
  border-bottom: 1px solid var(--border-default);
}

.label {
  font-size: var(--font-size-xs);
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-tertiary);
  margin-bottom: 3px;
}

.value { font-size: var(--font-size-sm); color: var(--text-primary); line-height: 1.5; word-break: break-word; user-select: text; -webkit-user-select: text; }
.value.mono { font-family: var(--font-mono); font-size: var(--font-size-xs); }
.value.small { font-size: var(--font-size-xs); }
.value.muted { color: var(--text-secondary); }

.tags { display: flex; flex-wrap: wrap; gap: 4px; }

.tag {
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  color: var(--text-secondary);
  padding: 1px 8px;
  border-radius: 12px;
  font-size: var(--font-size-xs);
}

.muted { color: var(--text-tertiary); }

/* Edit mode */
.input {
  width: 100%;
  box-sizing: border-box;
  padding: 5px 8px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  transition: border-color 0.12s;
}
.input:focus { border-color: var(--accent); outline: none; }
.input.mono { font-family: var(--font-mono); font-size: var(--font-size-xs); }
.flex1 { flex: 1; width: auto; }

.array-row {
  display: flex;
  gap: 5px;
  margin-bottom: 5px;
}

.rm-btn {
  flex-shrink: 0;
  width: 24px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-tertiary);
  font-size: 14px;
  cursor: pointer;
  transition: background 0.1s;
}
.rm-btn:hover { background: #fff0f0; color: #cc3333; border-color: #ffcccc; }

.add-btn {
  font-size: var(--font-size-xs);
  color: var(--accent);
  padding: 3px 0;
  cursor: pointer;
  background: none;
  border: none;
  text-align: left;
}
.add-btn:hover { text-decoration: underline; }

/* Tag chips (edit mode) */
.tag-chips { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 6px; }
.tag-chip {
  display: inline-flex; align-items: center; gap: 3px;
  background: var(--accent-light); color: var(--accent);
  border-radius: var(--radius-pill); padding: 2px 8px;
  font-size: var(--font-size-xs);
}
.tag-chip-rm {
  background: none; border: none; cursor: pointer;
  color: var(--accent); font-size: 13px; line-height: 1; padding: 0;
}
.tag-chip-rm:hover { color: #e53e3e; }

/* Tag autocomplete */
.tag-input-wrap { position: relative; }
.tag-suggestions {
  position: absolute; top: 100%; left: 0; right: 0; z-index: 50;
  background: var(--bg-primary); border: 1px solid var(--border-default);
  border-radius: var(--radius-sm); box-shadow: var(--shadow-md);
  max-height: 180px; overflow-y: auto;
  margin-top: 2px;
}
.tag-suggestion {
  display: block; width: 100%; text-align: left;
  padding: 5px 10px; font-size: var(--font-size-sm);
  color: var(--text-primary); cursor: pointer;
  transition: background 0.08s;
}
.tag-suggestion:hover { background: var(--accent); color: #fff; }

/* Reading status badge (read view) */
.status-badge {
  display: inline-flex; align-items: center;
  padding: 2px 9px; border-radius: var(--radius-pill);
  font-size: var(--font-size-xs); font-weight: 500;
}
.status-unread  { background: var(--bg-secondary); color: var(--text-tertiary); border: 1px solid var(--border-subtle); }
.status-reading { background: #fef3c7; color: #92400e; }
.status-read    { background: #dcfce7; color: #166534; }

/* Reading status buttons (edit view) */
.status-btns { display: flex; gap: 5px; }
.status-btn {
  flex: 1; padding: 4px 8px; font-size: var(--font-size-xs);
  border: 1px solid var(--border-default); border-radius: var(--radius-sm);
  background: var(--bg-secondary); color: var(--text-secondary); cursor: pointer;
  transition: all 0.1s;
}
.status-btn:hover { border-color: var(--accent); color: var(--accent); }
.status-btn.active { background: var(--accent); color: #fff; border-color: transparent; }

/* BibTeX section */
.bibtex-field { margin-top: 4px; border-top: 1px solid var(--border-default); padding-top: 10px; }
.bibtex-label-row {
  display: flex; align-items: center; justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
}
.bibtex-block {
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.55;
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  white-space: pre-wrap;
  word-break: break-all;
  overflow-x: auto;
  margin: 0;
}
.bibtex-textarea {
  width: 100%;
  box-sizing: border-box;
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.55;
  height: 160px;
  resize: vertical;
  padding: 8px 10px;
  border: 1px solid var(--accent);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-primary);
  outline: none;
}
.bibtex-edit-actions {
  display: flex;
  gap: 6px;
  margin-top: 6px;
}

/* Citation count section */
.cite-count-label-row {
  display: flex; align-items: center; justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
}
.cite-count-edit-row {
  display: flex;
  align-items: center;
  gap: 6px;
}
.cite-count-input {
  width: 110px;
}
.cite-count-val {
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}

/* Abstract section */
.abstract-field { margin-top: 4px; border-top: 1px solid var(--border-default); padding-top: 10px; }
.abstract-label-row {
  display: flex; align-items: center; justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
}
.section-actions {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  flex-shrink: 0;
}
.abstract-btn {
  display: inline-flex; align-items: center; gap: 4px;
  font-size: var(--font-size-xs); color: var(--accent);
  padding: 2px 8px; border-radius: var(--radius-pill);
  border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
  white-space: nowrap; cursor: pointer; transition: opacity 0.1s;
}
.abstract-btn:hover:not(:disabled) { opacity: 0.75; }
.abstract-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.copy-section-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  border: 1px solid var(--border-default);
  background: var(--bg-primary);
  white-space: nowrap;
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.copy-section-btn:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
  border-color: color-mix(in srgb, var(--accent) 28%, var(--border-default));
}
.copy-section-btn.done {
  color: #16803a;
  border-color: color-mix(in srgb, #16803a 28%, var(--border-default));
  background: color-mix(in srgb, #16803a 8%, transparent);
}
.copy-section-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.abstract-text {
  font-size: var(--font-size-sm); color: var(--text-primary);
  line-height: 1.6; white-space: pre-wrap; word-break: break-word;
}
.abstract-error { font-size: var(--font-size-xs); color: #dc2626; word-break: break-word; }
@keyframes spin-xs-anim { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
.spin-xs { animation: spin-xs-anim 0.8s linear infinite; flex-shrink: 0; }

/* Source field */
.source-field { }
.source-label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 6px;
}
.source-val { display: flex; align-items: center; }
.src-chip {
  display: inline-flex; align-items: center;
  padding: 2px 10px; border-radius: var(--radius-pill);
  font-size: var(--font-size-xs); font-weight: 500;
}
.src-arxiv { background: var(--source-arxiv-bg); color: var(--source-arxiv-text); }
.src-file  { background: var(--source-file-bg);  color: var(--source-file-text);  }
.src-url   { background: var(--source-url-bg);   color: var(--source-url-text);   }

.source-btns { display: flex; gap: 5px; }
.source-btn {
  flex: 1; padding: 4px 8px; font-size: var(--font-size-xs);
  border: 1px solid var(--border-default); border-radius: var(--radius-sm);
  background: var(--bg-secondary); color: var(--text-secondary); cursor: pointer;
  transition: all 0.1s;
}
.source-btn:hover { border-color: var(--accent); color: var(--accent); }
.source-btn.active { background: var(--accent); color: #fff; border-color: transparent; }
.source-edit-actions {
  display: flex;
  gap: 6px;
  margin-top: 6px;
}

/* Fulltext section */
.fulltext-field { margin-top: 4px; border-top: 1px solid var(--border-default); padding-top: 10px; }
.fulltext-label-row {
  display: flex; align-items: center; justify-content: space-between;
  gap: 8px;
  margin-bottom: 5px;
}
.fulltext-chars { font-size: var(--font-size-xs); color: var(--text-tertiary); font-weight: 400; text-transform: none; letter-spacing: 0; }
.fulltext-placeholder { font-size: var(--font-size-sm); padding: 4px 0; }
.fulltext-box {
  width: 100%;
  box-sizing: border-box;
  height: 220px;
  resize: vertical;
  padding: 8px;
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.55;
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-word;
}
.fulltext-box:focus { outline: none; border-color: var(--border-default); }
.fulltext-editor {
  height: 280px;
  color: var(--text-primary);
  background: var(--bg-primary);
  border-color: var(--accent-light);
}
.fulltext-editor:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 12%, transparent);
}
.fulltext-edit-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}
.fulltext-error {
  margin-top: 6px;
  font-size: var(--font-size-xs);
  color: #dc2626;
  word-break: break-word;
}
</style>
