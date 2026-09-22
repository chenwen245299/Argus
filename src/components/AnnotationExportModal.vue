<script setup lang="ts">
/**
 * Export the selected papers' highlights and notes.
 *
 * A dialog rather than a submenu: format × granularity is six combinations, and
 * a six-item submenu hides the one thing worth seeing before committing — how
 * much there actually is to export. Papers with nothing annotated are counted
 * separately here, because "3 篇" and "3 篇，其中 2 篇是空的" are different
 * decisions.
 */
import { ref, computed, onMounted } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { save as dialogSave, open as dialogOpen } from '@tauri-apps/plugin-dialog'
import type { ExportedPaper, AnnotationExportFile, AnnotationFolderExport } from '../types'

const props = defineProps<{ slugs: string[] }>()
const emit = defineEmits<{ close: [] }>()

const { t } = useI18n()

type Format = 'markdown' | 'json' | 'pdf'
const format = ref<Format>('markdown')
/**
 * `folder` writes a self-contained folder — highlights per paper, notes in
 * their own tree — and is the default because that is what an export of more
 * than one thing wants to be. `single` is the escape hatch for the user who
 * wants exactly one file to hand to someone.
 */
const granularity = ref<'folder' | 'single'>('folder')
/** PDF only: whether each paper starts on a fresh printed page. */
const pageBreaks = ref(false)

const loading = ref(true)
const busy = ref(false)
const error = ref('')
const done = ref('')
const papers = ref<ExportedPaper[]>([])

const totalHighlights = computed(() => papers.value.reduce((n, p) => n + p.highlights.length, 0))
const totalNotes = computed(() =>
  papers.value.reduce((n, p) => n + (p.notes ?? []).filter(x => x.content.trim()).length, 0)
)
const emptyCount = computed(() =>
  papers.value.filter(p => p.highlights.length === 0 && !(p.notes ?? []).some(n => n.content.trim())).length
)
const hasContent = computed(() => totalHighlights.value > 0 || totalNotes.value > 0)

async function run() {
  busy.value = true
  error.value = ''
  done.value = ''
  try {
    if (format.value === 'pdf') {
      // The preview window reads this and renders it; PDF comes out of the
      // system print dialog, where CJK actually works.
      localStorage.setItem(
        'argus:annotation-print',
        JSON.stringify({ slugs: props.slugs, pageBreaks: pageBreaks.value }),
      )
      await invoke('open_annotation_print_window')
      emit('close')
      return
    }

    if (granularity.value === 'folder') {
      // One folder dialog, and the backend writes the whole tree: the
      // directories have to exist before their contents, and a write per file
      // over IPC would leave half a folder behind if one of them failed.
      const parent = await dialogOpen({
        directory: true,
        multiple: false,
        title: t('annotationExport.chooseFolder'),
      })
      if (typeof parent !== 'string') return
      const out = await invoke<AnnotationFolderExport>('export_annotations_to_folder', {
        slugs: props.slugs,
        format: format.value,
        parent,
      })
      // The backend reveals the folder, so this only has to name it.
      done.value = t('annotationExport.savedFolder', { name: out.name, n: out.fileCount })
    } else {
      const file = await invoke<AnnotationExportFile>('export_annotations', {
        slugs: props.slugs,
        format: format.value,
      })
      const ext = format.value === 'json' ? 'json' : 'md'
      const path = await dialogSave({
        defaultPath: file.path,
        filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
      })
      if (!path) return
      await invoke('write_bytes_to_file', {
        path,
        bytes: Array.from(new TextEncoder().encode(file.content)),
      })
      done.value = t('annotationExport.saved')
    }
    setTimeout(() => emit('close'), 1200)
  } catch (e) {
    error.value = String(e)
  } finally {
    busy.value = false
  }
}

onMounted(async () => {
  try {
    papers.value = await invoke<ExportedPaper[]>('collect_annotations', { slugs: props.slugs })
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <div class="ae-backdrop" @click.self="emit('close')">
    <div class="ae-modal">
      <div class="ae-head">
        <Icon icon="fluent:arrow-export-24-regular" width="16" height="16" />
        <span>{{ t('annotationExport.title') }}</span>
        <button class="ae-close" @click="emit('close')">
          <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
        </button>
      </div>

      <div class="ae-body">
        <p v-if="loading" class="ae-note">{{ t('annotationExport.loading') }}</p>
        <template v-else>
          <p class="ae-counts">
            {{ t('annotationExport.counts', {
              papers: papers.length, highlights: totalHighlights, notes: totalNotes,
            }) }}
            <span v-if="emptyCount" class="ae-warn">
              {{ t('annotationExport.emptyCount', { n: emptyCount }) }}
            </span>
          </p>

          <div class="ae-field">
            <span class="ae-label">{{ t('annotationExport.format') }}</span>
            <div class="ae-chips">
              <button
                v-for="f in (['markdown', 'json', 'pdf'] as Format[])"
                :key="f"
                class="ae-chip"
                :class="{ active: format === f }"
                @click="format = f"
              >{{ t(`annotationExport.format_${f}`) }}</button>
            </div>
            <span class="ae-hint">{{ t(`annotationExport.formatHint_${format}`) }}</span>
          </div>

          <div v-if="format === 'pdf'" class="ae-field">
            <span class="ae-label">{{ t('annotationExport.layout') }}</span>
            <div class="ae-chips">
              <button class="ae-chip" :class="{ active: !pageBreaks }" @click="pageBreaks = false">
                {{ t('annotationExport.layoutFlow') }}
              </button>
              <button class="ae-chip" :class="{ active: pageBreaks }" @click="pageBreaks = true">
                {{ t('annotationExport.layoutPerPage') }}
              </button>
            </div>
          </div>

          <div v-else class="ae-field">
            <span class="ae-label">{{ t('annotationExport.granularity') }}</span>
            <div class="ae-chips">
              <button class="ae-chip" :class="{ active: granularity === 'folder' }" @click="granularity = 'folder'">
                {{ t('annotationExport.asFolder') }}
              </button>
              <button class="ae-chip" :class="{ active: granularity === 'single' }" @click="granularity = 'single'">
                {{ t('annotationExport.oneFile') }}
              </button>
            </div>
            <span class="ae-hint">
              {{ granularity === 'folder'
                 ? t(`annotationExport.folderHint_${format}`)
                 : t('annotationExport.oneFileHint') }}
            </span>
          </div>

          <p v-if="!hasContent" class="ae-warn block">{{ t('annotationExport.nothing') }}</p>
        </template>

        <p v-if="error" class="ae-error">{{ error }}</p>
        <p v-if="done" class="ae-done">{{ done }}</p>
      </div>

      <div class="ae-foot">
        <button class="ae-btn ghost" @click="emit('close')">{{ t('annotationExport.cancel') }}</button>
        <button class="ae-btn primary" :disabled="loading || busy || !hasContent" @click="run">
          {{ busy ? t('annotationExport.working')
             : format === 'pdf' ? t('annotationExport.openPreview')
             : granularity === 'folder' ? t('annotationExport.exportFolder')
             : t('annotationExport.export') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.ae-backdrop {
  position: fixed;
  inset: 0;
  z-index: 3000;
  background: rgba(0, 0, 0, 0.32);
  display: grid;
  place-items: center;
}
.ae-modal {
  width: min(420px, calc(100vw - 40px));
  border-radius: 12px;
  background: var(--bg-secondary, #fff);
  color: var(--text-primary, #1b1b1f);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.22);
  overflow: hidden;
  font-size: 13px;
}
.ae-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 13px 14px;
  border-bottom: 1px solid var(--border-color, #e4e6eb);
  font-weight: 600;
}
.ae-close {
  margin-left: auto;
  border: none;
  background: transparent;
  color: var(--text-secondary, #8a8f98);
  cursor: pointer;
  padding: 2px;
  display: grid;
  place-items: center;
}
.ae-body { padding: 14px; }
.ae-counts { color: var(--text-secondary, #6b7280); margin-bottom: 14px; line-height: 1.6; }
.ae-warn { color: #b45309; }
.ae-warn.block { margin-top: 10px; }
.ae-field { margin-bottom: 14px; display: flex; flex-direction: column; gap: 6px; }
.ae-label { font-size: 12px; color: var(--text-secondary, #6b7280); }
.ae-chips { display: flex; gap: 6px; flex-wrap: wrap; }
.ae-chip {
  padding: 6px 12px;
  border: 1px solid var(--border-color, #e4e6eb);
  border-radius: 7px;
  background: var(--bg-primary, #f7f8fa);
  color: inherit;
  font: inherit;
  cursor: pointer;
}
.ae-chip.active {
  border-color: var(--accent, #3b82f6);
  background: var(--accent-soft, rgba(80, 140, 255, 0.12));
  color: var(--accent, #3b82f6);
}
.ae-hint { font-size: 11.5px; color: var(--text-secondary, #8a8f98); line-height: 1.5; }
.ae-note { color: var(--text-secondary, #8a8f98); }
.ae-error { color: #c0392b; margin-top: 10px; line-height: 1.5; }
.ae-done { color: #15803d; margin-top: 10px; line-height: 1.5; word-break: break-all; }
.ae-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 14px;
  border-top: 1px solid var(--border-color, #e4e6eb);
}
.ae-btn { padding: 7px 14px; border-radius: 7px; border: 1px solid transparent; font: inherit; cursor: pointer; }
.ae-btn.ghost { background: var(--bg-primary, #f7f8fa); border-color: var(--border-color, #e4e6eb); color: inherit; }
.ae-btn.primary { background: var(--accent, #3b82f6); color: #fff; }
.ae-btn.primary:disabled { opacity: 0.45; cursor: not-allowed; }
</style>
