<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import type { PaperStatus } from '../types'

defineProps<{ status: PaperStatus; hasBibtex?: boolean | null }>()
const { t } = useI18n()
</script>

<template>
  <div class="badges">
    <span
      class="badge"
      :class="status.metadata_fetched ? 'badge--meta-ok' : 'badge--meta-missing'"
      :title="status.metadata_fetched ? t('badge.metaTitle') : t('badge.metaMissingTitle')"
    >
      <Icon :icon="status.metadata_fetched ? 'fluent:checkmark-24-regular' : 'fluent:dismiss-24-regular'" width="11" height="11" />
      {{ t('badge.meta') }}
    </span>
    <span
      v-if="status.text_extracted"
      class="badge badge--text-ok"
      :title="t('badge.textTitle')"
    >
      <Icon icon="fluent:checkmark-24-regular" width="11" height="11" />
      {{ t('badge.text') }}
    </span>
    <span
      v-if="status.ai_summary_done"
      class="badge badge--ai-ok"
      :title="t('badge.summaryTitle')"
    >
      <Icon icon="fluent:checkmark-24-regular" width="11" height="11" />
      AI
    </span>
    <span
      v-if="hasBibtex"
      class="badge badge--bibtex-ok"
      :title="t('badge.bibtexTitle')"
    >
      <Icon icon="fluent:checkmark-24-regular" width="11" height="11" />
      BibTeX
    </span>
  </div>
</template>

<style scoped>
.badges { display: flex; flex-wrap: nowrap; gap: 3px; }

.badge {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  font-size: 10px;
  font-weight: 500;
  padding: 1px 6px;
  border-radius: 10px;
  white-space: nowrap;
}

.badge--meta-ok {
  background: color-mix(in srgb, #22c55e 16%, transparent);
  color: #15803d;
}
.badge--meta-missing {
  background: color-mix(in srgb, #ef4444 16%, transparent);
  color: #dc2626;
}
.badge--text-ok {
  background: color-mix(in srgb, #22c55e 16%, transparent);
  color: #15803d;
}
.badge--ai-ok {
  background: color-mix(in srgb, #8b5cf6 14%, transparent);
  color: #7c3aed;
  font-weight: 600;
}
.badge--bibtex-ok {
  background: color-mix(in srgb, #0ea5e9 15%, transparent);
  color: #0369a1;
}
</style>
