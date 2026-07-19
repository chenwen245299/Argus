<script setup lang="ts">
import { computed, ref, watch, onMounted, onBeforeUnmount } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useSelectionStore } from '../stores/selection'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import MetadataTab from './tabs/MetadataTab.vue'
import NotesTab from './tabs/NotesTab.vue'
import HighlightsTab from './tabs/HighlightsTab.vue'
import SectionsTab from './tabs/SectionsTab.vue'
import AnalysisTab from './tabs/AnalysisTab.vue'
import TranslationHistoryTab from './tabs/TranslationHistoryTab.vue'
import DrawTab from './tabs/DrawTab.vue'
import type { PaperMeta } from '../types'

const props = defineProps<{ activeTab: string }>()
const emit = defineEmits<{ 'open-settings': []; 'update:activeTab': [tab: string] }>()

const { t } = useI18n()
const selection = useSelectionStore()
const library = useLibraryStore()
const reader = useReaderStore()
const meta = ref<PaperMeta | null>(null)
const activePaperSlug = computed(() => reader.openSlug ?? selection.selectedSlug)
const paperTab = computed(() => props.activeTab === 'library' ? 'notes' : props.activeTab)

// The AI (Analysis) tab is kept mounted so a running generation keeps streaming
// in the background even when another tab or the canvas is shown. `aiSlug` is
// sticky — it only updates to a new *non-null* paper, so navigating to the
// canvas (which transiently clears the active paper) never remounts the tab and
// interrupts an in-flight answer.
const aiSlug = ref<string | null>(null)
watch(activePaperSlug, (s) => { if (s) aiSlug.value = s }, { immediate: true })

watch(activePaperSlug, async (slug) => {
  meta.value = null
  if (!slug) return
  try {
    meta.value = await invoke<PaperMeta>('get_paper_meta', { slug })
  } catch (e) {
    console.error('Failed to load meta:', e)
  }
}, { immediate: true })


async function onMetaSaved(updatedMeta: PaperMeta) {
  meta.value = updatedMeta
  await library.refresh()
}

function onExternalMetaUpdated(e: Event) {
  const detail = (e as CustomEvent<{ slug: string; meta: PaperMeta }>).detail
  if (detail?.slug === activePaperSlug.value) {
    meta.value = detail.meta
  }
}

onMounted(() => {
  window.addEventListener('argus-paper-meta-updated', onExternalMetaUpdated)
})

onBeforeUnmount(() => {
  window.removeEventListener('argus-paper-meta-updated', onExternalMetaUpdated)
})

async function onCanvasNotesUpdated(noteIds: string[]) {
  if (!activePaperSlug.value || !meta.value) return
  await invoke('set_canvas_notes', { slug: activePaperSlug.value, noteIds })
  meta.value = { ...meta.value, canvas_notes: noteIds }
  window.dispatchEvent(new CustomEvent('argus-canvas-notes-updated', {
    detail: { slug: activePaperSlug.value, noteIds },
  }))
}

async function onSlugChanged(newSlug: string) {
  const oldSlug = activePaperSlug.value
  // Update reader if the renamed paper was open
  if (oldSlug && reader.openSlug === oldSlug) {
    reader.replacePaperSlug(oldSlug, newSlug, meta.value?.title)
  }
  selection.selectPaper(newSlug)
  await library.refresh()
  // Reload meta with new slug
  try {
    meta.value = await invoke<PaperMeta>('get_paper_meta', { slug: newSlug })
  } catch {
    meta.value = null
  }
}
</script>

<template>
  <div class="right-sidebar">
    <!-- AI (Analysis) tab: kept mounted so a running generation keeps streaming
         in the background even when another tab or the canvas is shown. Only its
         visibility is toggled here. -->
    <div v-show="paperTab === 'ai' && !!activePaperSlug" class="tab-content">
      <AnalysisTab
        v-if="aiSlug"
        :slug="aiSlug"
        @open-settings="emit('open-settings')"
      />
    </div>

    <!-- Drawing properties: canvas-scoped, independent of paper selection -->
    <DrawTab v-if="props.activeTab === 'draw'" />

    <!-- Translation history: always available regardless of paper selection -->
    <TranslationHistoryTab v-else-if="paperTab === 'translations'" />

    <template v-else-if="!activePaperSlug">
      <!-- No paper selected -->
      <div class="no-selection">
        <Icon icon="fluent:book-24-regular" width="28" height="28" />
        <p>{{ t('sidebar.selectPaper') }}</p>
      </div>
    </template>

    <!-- Paper selected (the AI tab is handled by the persistent block above) -->
    <template v-else>
      <div v-show="paperTab !== 'ai'" class="tab-content">
        <MetadataTab
          v-if="paperTab === 'metadata'"
          :slug="activePaperSlug"
          :meta="meta"
          @saved="onMetaSaved"
          @slug-changed="onSlugChanged"
        />
        <NotesTab
          v-else-if="paperTab === 'notes'"
          :slug="activePaperSlug"
          :canvas-notes="meta?.canvas_notes ?? []"
          @update:canvas-notes="onCanvasNotesUpdated"
        />
        <HighlightsTab v-else-if="paperTab === 'highlights'" />
        <SectionsTab v-else-if="paperTab === 'sections'" :slug="activePaperSlug" />
      </div>
    </template>
  </div>
</template>

<style scoped>
.right-sidebar {
  display: flex;
  flex-direction: column;
  background: var(--bg-primary);
  overflow: hidden;
  height: 100%;
}

.no-selection {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: var(--text-tertiary);
}
.no-selection p { font-size: var(--font-size-sm); color: var(--text-secondary); }

.tab-content {
  flex: 1;
  overflow: hidden;
  display: flex;
  flex-direction: column;
}
</style>
