<script setup lang="ts">
// The approval card for a write the agent wants to make. Sits directly above
// the composer — the same width as the input box — rather than in the middle of
// the screen, so the request reads as part of the conversation you are in.
//
// It shows exactly what would be written: which paper, the note's title, and
// the body rendered as markdown, read-only and scrollable past ~10 lines.
// Nothing is written unless the user presses the confirm button; closing the
// window, stopping the answer or simply ignoring it all leave the library
// untouched (the backend treats silence as a refusal after its timeout).
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import MarkdownBody from './MarkdownBody.vue'
import type { AgentWritePreview } from '../types'

const props = defineProps<{
  preview: AgentWritePreview
  /** How many further confirmations are queued behind this one. */
  queued?: number
}>()

const emit = defineEmits<{ approve: []; reject: [] }>()

const { t } = useI18n()
</script>

<template>
  <div class="write-confirm">
    <div class="wc-head">
      <Icon class="wc-icon" icon="doodle:notebook" width="20" height="20" />
      <span class="wc-title">{{ t('writeConfirm.title') }}</span>
      <span v-if="props.queued" class="wc-queued">{{ t('writeConfirm.queued', { n: props.queued }) }}</span>
    </div>

    <dl class="wc-fields">
      <div class="wc-field">
        <dt>{{ t('writeConfirm.paper') }}</dt>
        <dd :title="preview.paperTitle">{{ preview.paperTitle }}</dd>
      </div>
      <div class="wc-field">
        <dt>{{ t('writeConfirm.noteTitle') }}</dt>
        <dd :title="preview.noteTitle">{{ preview.noteTitle }}</dd>
      </div>
      <div class="wc-field">
        <dt>{{ t('writeConfirm.location') }}</dt>
        <dd class="wc-location">
          {{ t('writeConfirm.locationValue', { paper: preview.paperTitle }) }}
        </dd>
      </div>
    </dl>

    <p v-if="preview.duplicateTitle" class="wc-warn">
      <Icon icon="fluent:warning-24-regular" width="13" height="13" />
      <span>{{ t('writeConfirm.duplicate') }}</span>
    </p>

    <!-- Read-only preview of the note body, capped at roughly ten lines. -->
    <div class="wc-body">
      <MarkdownBody :content="preview.content" />
    </div>

    <div class="wc-actions">
      <span class="wc-hint">{{ t('writeConfirm.hint') }}</span>
      <button class="wc-btn" @click="emit('reject')">{{ t('writeConfirm.reject') }}</button>
      <button class="wc-btn primary" @click="emit('approve')">
        <Icon icon="fluent:checkmark-24-regular" width="13" height="13" />
        {{ t('writeConfirm.approve') }}
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
