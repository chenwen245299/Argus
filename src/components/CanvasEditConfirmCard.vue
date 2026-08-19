<script setup lang="ts">
// The approval card for an AI edit to the canvas. Sits above the chat composer,
// exactly where the note card does — but it deliberately shows no body preview,
// because the preview is the change itself, drawn on the canvas behind this
// panel (green = added, amber = changed, red = to be removed). The card is the
// legend and the yes/no: it names how much changes and what the colours mean,
// and nothing is saved until the user presses confirm.
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import type { CanvasEditPreview } from '../types'

const props = defineProps<{
  preview: CanvasEditPreview
  /** How many further confirmations are queued behind this one. */
  queued?: number
}>()

const emit = defineEmits<{ approve: []; reject: [] }>()

const { t } = useI18n()

const s = computed(() => props.preview.summary)
const addedCount = computed(() => s.value.addedNodes + s.value.addedEdges)
const updatedCount = computed(() => s.value.updatedNodes + s.value.updatedEdges)
const removedCount = computed(() => s.value.removedNodes + s.value.removedEdges)

/** The colour a detail line belongs to, read from its leading glyph so the list
 *  is coloured the same way the canvas preview is. */
function lineKind(line: string): 'add' | 'update' | 'delete' | '' {
  if (line.startsWith('＋')) return 'add'
  if (line.startsWith('✎')) return 'update'
  if (line.startsWith('✕')) return 'delete'
  return ''
}
</script>

<template>
  <div class="ce-confirm">
    <div class="ce-head">
      <Icon class="ce-icon" icon="doodle:sparkle-trio" width="22" height="22" />
      <span class="ce-title">{{ t('canvasEdit.title') }}</span>
      <span v-if="props.queued" class="ce-queued">{{ t('canvasEdit.queued', { n: props.queued }) }}</span>
    </div>

    <!-- The preview lives on the canvas, not here — point the user at it. -->
    <p class="ce-look">
      <Icon icon="doodle:curved-arrow" width="18" height="18" />
      <span>{{ t('canvasEdit.previewHint', { name: preview.canvasName }) }}</span>
    </p>

    <!-- How much changes, by colour category. -->
    <div class="ce-tally">
      <span v-if="addedCount" class="ce-chip add">
        <i class="ce-swatch add" />{{ t('canvasEdit.added', { n: addedCount }) }}
      </span>
      <span v-if="updatedCount" class="ce-chip update">
        <i class="ce-swatch update" />{{ t('canvasEdit.updated', { n: updatedCount }) }}
      </span>
      <span v-if="removedCount" class="ce-chip delete">
        <i class="ce-swatch delete" />{{ t('canvasEdit.removed', { n: removedCount }) }}
      </span>
    </div>

    <!-- The operations, one per line, coloured to match the canvas. -->
    <ul class="ce-lines">
      <li v-for="(line, i) in s.lines" :key="i" :class="lineKind(line)">{{ line }}</li>
    </ul>

    <div class="ce-actions">
      <span class="ce-hint">{{ t('canvasEdit.hint') }}</span>
      <button class="ce-btn" @click="emit('reject')">{{ t('canvasEdit.reject') }}</button>
      <button class="ce-btn primary" @click="emit('approve')">
        <Icon icon="fluent:checkmark-24-regular" width="13" height="13" />
        {{ t('canvasEdit.approve') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.ce-confirm {
  display: flex;
  flex-direction: column;
  gap: 9px;
  padding: 12px 14px;
  margin-bottom: 8px;
  border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--accent) 6%, var(--bg-primary));
  box-shadow: var(--shadow-sm);
}

.ce-head { display: flex; align-items: center; gap: 8px; }
.ce-icon { color: var(--accent); flex-shrink: 0; }
.ce-title { font-size: 13px; font-weight: 650; color: var(--text-primary); }
.ce-queued {
  margin-left: auto;
  font-size: 11px;
  color: var(--text-tertiary);
  white-space: nowrap;
}

/* The nudge toward the canvas, set in the hand-drawn face to read as an aside. */
.ce-look {
  display: flex;
  align-items: center;
  gap: 7px;
  margin: 0;
  font-family: var(--font-hand);
  font-size: 12.5px;
  line-height: 1.5;
  color: var(--text-secondary);
}
.ce-look svg { color: color-mix(in srgb, var(--accent) 70%, var(--text-tertiary)); flex-shrink: 0; }

.ce-tally { display: flex; flex-wrap: wrap; gap: 6px; }
.ce-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 9px;
  font-size: 11.5px;
  font-weight: 600;
  border-radius: 999px;
}
.ce-chip.add { color: #15803d; background: color-mix(in srgb, #22c55e 15%, transparent); }
.ce-chip.update { color: #b45309; background: color-mix(in srgb, #f59e0b 16%, transparent); }
.ce-chip.delete { color: #dc2626; background: color-mix(in srgb, #ef4444 14%, transparent); }
.ce-swatch { width: 10px; height: 10px; border-radius: 3px; flex-shrink: 0; }
.ce-swatch.add { border: 1.5px dashed #22c55e; background: color-mix(in srgb, #22c55e 20%, transparent); }
.ce-swatch.update { border: 1.5px solid #f59e0b; background: transparent; }
.ce-swatch.delete { border: 1.5px solid #ef4444; background: color-mix(in srgb, #ef4444 14%, transparent); }

.ce-lines {
  margin: 0;
  padding: 8px 10px;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 3px;
  max-height: 150px;
  overflow-y: auto;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
}
.ce-lines li {
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.ce-lines li.add { color: #15803d; }
.ce-lines li.update { color: #b45309; }
.ce-lines li.delete { color: #dc2626; }

.ce-actions { display: flex; align-items: center; gap: 8px; }
.ce-hint { flex: 1; min-width: 0; font-size: 11px; color: var(--text-tertiary); }
.ce-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 13px;
  font-size: 12px;
  font-weight: 550;
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  cursor: pointer;
}
.ce-btn:hover { color: var(--text-primary); background: var(--bg-hover); }
.ce-btn.primary {
  color: #fff;
  background: var(--accent);
  border-color: transparent;
}
.ce-btn.primary:hover { filter: brightness(1.06); }
</style>
