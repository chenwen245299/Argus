<script setup lang="ts">
import { useI18n } from 'vue-i18n'
import type { PaperStatus } from '../types'

defineProps<{ status: PaperStatus }>()
const { t } = useI18n()
</script>

<template>
  <div class="badges">
    <span
      class="badge"
      :class="status.metadata_fetched ? 'badge--meta-ok' : 'badge--meta-missing'"
      :title="status.metadata_fetched ? t('badge.metaTitle') : t('badge.metaMissingTitle')"
    >
      <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <polyline v-if="status.metadata_fetched" points="20 6 9 17 4 12"/>
        <template v-else>
          <line x1="18" y1="6" x2="6" y2="18"/>
          <line x1="6" y1="6" x2="18" y2="18"/>
        </template>
      </svg>
      {{ t('badge.meta') }}
    </span>
    <span
      v-if="status.text_extracted"
      class="badge badge--text-ok"
      :title="t('badge.textTitle')"
    >
      <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <polyline points="20 6 9 17 4 12"/>
      </svg>
      {{ t('badge.text') }}
    </span>
    <span
      v-if="status.ai_summary_done"
      class="badge badge--ai-ok"
      :title="t('badge.summaryTitle')"
    >
      <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <polyline points="20 6 9 17 4 12"/>
      </svg>
      AI
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
</style>
