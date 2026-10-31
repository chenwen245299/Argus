<script setup lang="ts">
// The approval card for a write the agent wants to make. Sits directly above
// the composer — the same width as the input box — rather than in the middle of
// the screen, so the request reads as part of the conversation you are in.
//
// It shows exactly what would be written: which paper, the note's title, and
// the body rendered as markdown, read-only and scrollable past ~10 lines. A
// change to existing marks shows each one before and after (or, for a
// retraction, what disappears), and whose mark it is.
// Nothing is written unless the user presses the confirm button; closing the
// window, stopping the answer or simply ignoring it all leave the library
// untouched (the backend treats silence as a refusal after its timeout).
import { useI18n } from 'vue-i18n'
import { computed, ref } from 'vue'
import { Icon } from '@iconify/vue'
import MarkdownBody from './MarkdownBody.vue'
import type { AgentWritePreview } from '../types'
import { HIGHLIGHT_COLORS } from '../utils/highlightColors'

const props = defineProps<{
  preview: AgentWritePreview
  /** How many further confirmations are queued behind this one. */
  queued?: number
}>()

const emit = defineEmits<{ approve: [colors?: string[]]; reject: [] }>()

const { t } = useI18n()
const noteAction = computed(() => ({ update_paper_note: 'Update', delete_paper_note: 'Delete', restore_paper_note: 'Restore' } as Record<string, string>)[props.preview.tool])
const recommendationAction = computed(() => props.preview.recommendations?.length
  ? (props.preview.tool === 'delete_arxiv_recommendations' ? 'delete' : 'import') : null)
const changeAction = computed(() => props.preview.changes?.length
  ? (props.preview.tool === 'remove_paper_highlights' ? 'remove' : 'update') : null)
const marking = computed(() => !!props.preview.passages?.length || !!changeAction.value)
// One choice per new passage, or per changed mark's new color. A retraction
// has nothing to choose.
const selectedColors = ref(props.preview.passages?.length
  ? props.preview.passages.map(p => p.color)
  : props.preview.changes?.map(c => c.after?.color ?? c.before.color) ?? [])

const sameColor = (a?: string | null, b?: string | null) => (a ?? '').toUpperCase() === (b ?? '').toUpperCase()
function colorName(color: string) {
  const known = HIGHLIGHT_COLORS.find(c => sameColor(c.value, color))
  return known ? t(known.labelKey) : color.toUpperCase()
}
function changeDiff(index: number) {
  const change = props.preview.changes![index]
  const after = change.after!
  return {
    color: !sameColor(selectedColors.value[index], change.before.color),
    style: after.style !== change.before.style,
    note: (after.note ?? '') !== (change.before.note ?? ''),
  }
}
const styleName = (style: string) => t(style === 'underline' ? 'writeConfirm.underline' : 'writeConfirm.highlight')

function approve() {
  const recolorable = !!props.preview.passages?.length || changeAction.value === 'update'
  emit('approve', recolorable ? [...selectedColors.value] : undefined)
}
</script>

<template>
  <div class="write-confirm">
    <div class="wc-head">
      <Icon class="wc-icon" :icon="noteAction ? (noteAction === 'Delete' ? 'fluent:delete-24-regular' : noteAction === 'Restore' ? 'fluent:arrow-counterclockwise-24-regular' : 'fluent:edit-24-regular') : recommendationAction ? (recommendationAction === 'delete' ? 'fluent:delete-24-regular' : 'fluent:library-24-regular') : changeAction === 'remove' ? 'fluent:delete-24-regular' : changeAction ? 'fluent:edit-24-regular' : preview.passages?.length ? 'fluent:highlight-24-regular' : 'doodle:notebook'" width="20" height="20" />
      <span class="wc-title">{{ t(noteAction ? `writeConfirm.note${noteAction}Title` : recommendationAction ? `writeConfirm.arxiv${recommendationAction === 'delete' ? 'Delete' : 'Import'}Title` : changeAction ? `writeConfirm.mark${changeAction === 'remove' ? 'Remove' : 'Update'}Title` : preview.passages?.length ? 'writeConfirm.markTitle' : 'writeConfirm.title') }}</span>
      <span v-if="props.queued" class="wc-queued">{{ t('writeConfirm.queued', { n: props.queued }) }}</span>
    </div>

    <dl v-if="recommendationAction" class="wc-fields">
      <div class="wc-field"><dt>{{ t('writeConfirm.paper') }}</dt><dd>{{ t('writeConfirm.arxivCount', { n: preview.recommendations?.length }) }}</dd></div>
      <div class="wc-field"><dt>{{ t('writeConfirm.location') }}</dt><dd class="wc-location">{{ t(recommendationAction === 'delete' ? 'writeConfirm.arxivDeleteLocation' : 'writeConfirm.arxivImportLocation') }}</dd></div>
    </dl>
    <dl v-else class="wc-fields">
      <div class="wc-field">
        <dt>{{ t('writeConfirm.paper') }}</dt>
        <dd :title="preview.slug">{{ preview.paperTitle }}</dd>
      </div>
      <div v-if="!marking" class="wc-field">
        <dt>{{ t('writeConfirm.noteTitle') }}</dt>
        <dd :title="preview.noteTitle">{{ preview.noteTitle }}</dd>
      </div>
      <div class="wc-field">
        <dt>{{ t('writeConfirm.location') }}</dt>
        <dd class="wc-location">
          {{ marking ? preview.slug : noteAction ? t(noteAction === 'Delete' ? 'writeConfirm.noteTrashLocation' : 'writeConfirm.noteExistingLocation') : t('writeConfirm.locationValue', { paper: preview.paperTitle }) }}
        </dd>
      </div>
    </dl>

    <p v-if="preview.duplicateTitle" class="wc-warn">
      <Icon icon="fluent:warning-24-regular" width="13" height="13" />
      <span>{{ t('writeConfirm.duplicate') }}</span>
    </p>

    <!-- Read-only preview of the note body, capped at roughly ten lines. -->
    <div v-if="recommendationAction" class="wc-body">
      <div v-for="paper in preview.recommendations" :key="paper.arxivId" class="wc-recommendation">
        <div class="wc-paper-title">{{ paper.title }}</div>
        <div class="wc-page">{{ paper.source === 'biorxiv' ? 'bioRxiv' : 'arXiv' }} · {{ paper.arxivId }}</div>
        <p v-if="paper.existingSlug" class="wc-warn">{{ t(preview.keepDuplicateCopies ? 'writeConfirm.arxivKeepDuplicate' : 'writeConfirm.arxivSkipDuplicate', { title: paper.existingTitle || paper.existingSlug }) }}</p>
      </div>
      <p v-if="recommendationAction === 'import'" class="wc-page">{{ t(preview.keepDuplicateCopies ? 'writeConfirm.arxivKeepPolicy' : 'writeConfirm.arxivSkipPolicy') }}</p>
    </div>
    <div v-else-if="changeAction" class="wc-body">
      <div v-for="(change, index) in preview.changes" :key="change.id" class="wc-passage wc-change">
        <div class="wc-page">
          {{ change.pageEnd && change.pageEnd !== change.page ? t('writeConfirm.pageRange', { n: change.page, m: change.pageEnd }) : t('writeConfirm.page', { n: change.page }) }}
          · {{ styleName(change.before.style) }}
          · <span class="wc-origin" :class="`wc-origin-${change.origin}`">{{ t(`writeConfirm.origin.${change.origin}`) }}</span>
        </div>
        <template v-if="change.after">
          <div class="wc-colors" :aria-label="t('writeConfirm.color')" role="group">
            <span>{{ t('writeConfirm.color') }}</span>
            <button v-for="color in HIGHLIGHT_COLORS" :key="color.value" type="button" class="wc-color"
              :title="t(color.labelKey)" :aria-label="t(color.labelKey)"
              :aria-pressed="selectedColors[index]?.toUpperCase() === color.value"
              :style="{ background: color.value }" @click="selectedColors[index] = color.value">
              <Icon v-if="selectedColors[index]?.toUpperCase() === color.value" icon="fluent:checkmark-16-regular" width="12" height="12" />
            </button>
            <label class="wc-custom-color" :title="t('writeConfirm.customColor')">
              <input v-model="selectedColors[index]" type="color" :aria-label="t('writeConfirm.customColor')" />
            </label>
          </div>
          <blockquote :style="{ borderColor: selectedColors[index] }">{{ change.text }}</blockquote>
          <dl class="wc-diff">
            <div v-if="changeDiff(index).color" class="wc-diff-row">
              <dt>{{ t('writeConfirm.color') }}</dt>
              <dd>
                <span class="wc-swatch" :style="{ background: change.before.color }" />{{ colorName(change.before.color) }}
                <span class="wc-arrow">→</span>
                <span class="wc-swatch" :style="{ background: selectedColors[index] }" />{{ colorName(selectedColors[index]) }}
              </dd>
            </div>
            <div v-if="changeDiff(index).style" class="wc-diff-row">
              <dt>{{ t('writeConfirm.style') }}</dt>
              <dd>{{ styleName(change.before.style) }}<span class="wc-arrow">→</span>{{ styleName(change.after.style) }}</dd>
            </div>
            <div v-if="changeDiff(index).note" class="wc-diff-row wc-diff-note">
              <dt>{{ t('writeConfirm.note') }}</dt>
              <dd>
                <div class="wc-note-before">
                  <span class="wc-tag">{{ t('writeConfirm.before') }}</span>
                  <MarkdownBody v-if="change.before.note" :content="change.before.note" />
                  <span v-else class="wc-empty">{{ t('writeConfirm.noNote') }}</span>
                </div>
                <div class="wc-note-after">
                  <span class="wc-tag">{{ t('writeConfirm.after') }}</span>
                  <MarkdownBody v-if="change.after.note" :content="change.after.note" />
                  <span v-else class="wc-empty">{{ t('writeConfirm.noteRemoved') }}</span>
                </div>
              </dd>
            </div>
          </dl>
          <p v-if="!changeDiff(index).color && !changeDiff(index).style && !changeDiff(index).note" class="wc-page">{{ t('writeConfirm.noChange') }}</p>
        </template>
        <template v-else>
          <blockquote :style="{ borderColor: change.before.color }">{{ change.text }}</blockquote>
          <MarkdownBody v-if="change.before.note" :content="change.before.note" />
        </template>
      </div>
    </div>
    <div v-else-if="preview.passages?.length" class="wc-body">
      <div v-for="(passage, index) in preview.passages" :key="index" class="wc-passage">
        <div class="wc-page">{{ t('writeConfirm.page', { n: passage.page }) }} · {{ t(passage.style === 'underline' ? 'writeConfirm.underline' : 'writeConfirm.highlight') }}</div>
        <div class="wc-colors" :aria-label="t('writeConfirm.color')" role="group">
          <span>{{ t('writeConfirm.color') }}</span>
          <button v-for="color in HIGHLIGHT_COLORS" :key="color.value" type="button" class="wc-color"
            :title="t(color.labelKey)" :aria-label="t(color.labelKey)"
            :aria-pressed="selectedColors[index]?.toUpperCase() === color.value"
            :style="{ background: color.value }" @click="selectedColors[index] = color.value">
            <Icon v-if="selectedColors[index]?.toUpperCase() === color.value" icon="fluent:checkmark-16-regular" width="12" height="12" />
          </button>
          <label class="wc-custom-color" :title="t('writeConfirm.customColor')">
            <input v-model="selectedColors[index]" type="color" :aria-label="t('writeConfirm.customColor')" />
          </label>
        </div>
        <blockquote :style="{ borderColor: selectedColors[index] }">{{ passage.text }}</blockquote>
        <MarkdownBody v-if="passage.note" :content="passage.note" />
      </div>
    </div>
    <div v-else class="wc-body">
      <template v-if="noteAction === 'Update' && preview.noteBefore">
        <div class="wc-note-version wc-note-before">
          <span class="wc-version-label">{{ t('writeConfirm.before') }}</span>
          <strong>{{ preview.noteBefore.title }}</strong>
          <MarkdownBody v-if="preview.noteBefore.content" :content="preview.noteBefore.content" />
          <span v-else class="wc-empty">{{ t('writeConfirm.emptyNote') }}</span>
        </div>
        <div class="wc-note-version wc-note-after">
          <span class="wc-version-label">{{ t('writeConfirm.after') }}</span>
          <strong>{{ preview.noteTitle }}</strong>
          <MarkdownBody v-if="preview.content" :content="preview.content" />
          <span v-else class="wc-empty">{{ t('writeConfirm.emptyNote') }}</span>
        </div>
      </template>
      <template v-else>
        <MarkdownBody v-if="preview.content" :content="preview.content" />
        <span v-else class="wc-empty">{{ t('writeConfirm.emptyNote') }}</span>
      </template>
    </div>

    <div class="wc-actions">
      <span class="wc-hint">{{ t(noteAction ? `writeConfirm.note${noteAction}Hint` : recommendationAction ? 'writeConfirm.arxivHint' : changeAction ? `writeConfirm.${changeAction}Hint` : 'writeConfirm.hint') }}</span>
      <button class="wc-btn" @click="emit('reject')">{{ t(noteAction || recommendationAction ? 'writeConfirm.arxivReject' : changeAction ? `writeConfirm.${changeAction}Reject` : 'writeConfirm.reject') }}</button>
      <button class="wc-btn primary" :class="{ danger: noteAction === 'Delete' || recommendationAction === 'delete' || changeAction === 'remove' }" @click="approve">
        <Icon icon="fluent:checkmark-24-regular" width="13" height="13" />
        {{ t(noteAction ? `writeConfirm.note${noteAction}Approve` : recommendationAction ? `writeConfirm.arxiv${recommendationAction === 'delete' ? 'Delete' : 'Import'}Approve` : changeAction ? `writeConfirm.${changeAction}Approve` : 'writeConfirm.approve') }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.write-confirm {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px 14px;
  margin-bottom: 8px;
  border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--accent) 6%, var(--bg-primary));
  box-shadow: var(--shadow-sm);
}

.wc-head { display: flex; align-items: center; gap: 7px; }
.wc-icon { color: var(--accent); flex-shrink: 0; }
.wc-title { font-size: 13px; font-weight: 600; color: var(--text-primary); }
.wc-queued {
  margin-left: auto;
  font-size: 11px;
  color: var(--text-tertiary);
  white-space: nowrap;
}

.wc-fields { margin: 0; display: flex; flex-direction: column; gap: 3px; }
.wc-field { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.wc-field dt {
  flex-shrink: 0;
  width: 56px;
  font-size: 11.5px;
  color: var(--text-tertiary);
}
.wc-field dd {
  margin: 0;
  min-width: 0;
  font-size: 12.5px;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.wc-location { color: var(--text-secondary) !important; }

.wc-warn {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 11.5px;
  line-height: 1.5;
  color: #b45309;
}
.wc-warn svg { flex-shrink: 0; }

/* ~10 lines, then it scrolls. The note itself may be far longer; this is a
   preview of what will be written, not an editor. */
.wc-body {
  max-height: 220px;
  overflow-y: auto;
  padding: 10px 12px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  font-size: 12.5px;
  line-height: 1.65;
  color: var(--text-primary);
  user-select: text;
}
.wc-body :deep(> div > *:first-child) { margin-top: 0; }
.wc-body :deep(> div > *:last-child) { margin-bottom: 0; }
.wc-body :deep(h1), .wc-body :deep(h2), .wc-body :deep(h3) { font-size: 13.5px; margin: 10px 0 6px; }
.wc-body :deep(p) { margin: 6px 0; }
.wc-body :deep(ul), .wc-body :deep(ol) { margin: 6px 0; padding-left: 20px; }
.wc-body :deep(pre) { font-size: 11.5px; overflow-x: auto; }
.wc-body :deep(img) { max-width: 100%; }
.wc-passage + .wc-passage { border-top: 1px solid var(--border-subtle); margin-top: 10px; padding-top: 10px; }
.wc-page { color: var(--text-tertiary); font-size: 11px; }
.wc-recommendation + .wc-recommendation { border-top: 1px solid var(--border-subtle); margin-top: 8px; padding-top: 8px; }
.wc-paper-title { font-weight: 550; overflow-wrap: anywhere; }
.wc-recommendation .wc-page { overflow-wrap: anywhere; }
.wc-btn.primary.danger { background: var(--color-danger, #dc2626); }
.wc-colors { display: flex; align-items: center; gap: 7px; margin: 6px 0; }
.wc-colors > span { margin-right: 2px; font-size: 11px; color: var(--text-tertiary); }
.wc-color { display: grid; place-items: center; width: 20px; height: 20px; padding: 0; border: 1px solid rgba(0,0,0,.12); border-radius: 50%; color: #263238; cursor: pointer; }
.wc-color[aria-pressed="true"] { outline: 2px solid var(--accent); outline-offset: 2px; }
.wc-color:focus-visible, .wc-custom-color input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.wc-custom-color { display: flex; align-items: center; width: 24px; height: 22px; margin-left: 2px; }
.wc-custom-color input { width: 24px; height: 22px; padding: 1px; border: 1px solid var(--border-default); border-radius: 5px; background: var(--bg-primary); cursor: pointer; }
.wc-passage blockquote { border-left: 4px solid; padding: 3px 9px; margin: 5px 0; white-space: pre-wrap; }
.wc-origin { color: var(--text-secondary); }
.wc-origin-user, .wc-origin-ai_user_edited { color: #b45309; }
.wc-diff { display: flex; flex-direction: column; gap: 4px; margin: 6px 0 0; }
.wc-diff-row { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.wc-diff-row dt { flex-shrink: 0; width: 40px; font-size: 11px; color: var(--text-tertiary); }
.wc-diff-row dd { display: flex; align-items: center; flex-wrap: wrap; gap: 4px; margin: 0; min-width: 0; }
.wc-diff-note dd { display: block; flex: 1; }
.wc-swatch { display: inline-block; width: 11px; height: 11px; border: 1px solid rgba(0,0,0,.12); border-radius: 50%; }
.wc-arrow { margin: 0 4px; color: var(--text-tertiary); }
.wc-note-version { display: flex; flex-direction: column; gap: 5px; }
.wc-version-label { color: var(--text-tertiary); font-size: 11px; }
.wc-note-before, .wc-note-after { padding: 2px 0 2px 8px; border-left: 2px solid var(--border-subtle); }
.wc-note-before { color: var(--text-secondary); opacity: .8; }
.wc-note-after { margin-top: 4px; border-left-color: var(--accent); }
.wc-tag { display: block; font-size: 10.5px; color: var(--text-tertiary); }
.wc-empty { color: var(--text-tertiary); font-style: italic; }

.wc-actions { display: flex; align-items: center; gap: 8px; }
.wc-hint { flex: 1; min-width: 0; font-size: 11px; color: var(--text-tertiary); }
.wc-btn {
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
.wc-btn:hover { color: var(--text-primary); background: var(--bg-hover); }
.wc-btn.primary {
  color: #fff;
  background: var(--accent);
  border-color: transparent;
}
.wc-btn.primary:hover { filter: brightness(1.06); }
</style>
