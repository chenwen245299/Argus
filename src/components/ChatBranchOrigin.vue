<script setup lang="ts">
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import type { ConversationBranch } from '../utils/chatBranches'
defineProps<{ branch: ConversationBranch; parentAvailable: boolean; parentTitle?: string }>()
defineEmits<{ open: [] }>()
const { t } = useI18n()
</script>

<template>
  <div class="branch-origin">
    <Icon icon="fluent:branch-fork-24-regular" width="16" height="16" />
    <div class="branch-origin-copy">
      <span>{{ t('chat.branchOrigin', { turn: branch.turn }) }}</span>
      <button v-if="parentAvailable" type="button" :title="t('chat.branchOpenParent')" @click="$emit('open')">
        {{ parentTitle || branch.parentTitle }}
        <Icon icon="fluent:arrow-up-right-24-regular" width="12" height="12" />
      </button>
      <span v-else class="branch-missing">{{ branch.parentTitle }} · {{ t('chat.branchParentDeleted') }}</span>
    </div>
  </div>
</template>

<style scoped>
.branch-origin {
  display: flex;
  align-items: center;
  gap: 9px;
  margin: 0 0 24px;
  padding: 10px 12px;
  border: 1px solid color-mix(in srgb, var(--accent) 18%, var(--border-subtle));
  border-radius: var(--radius-md);
  background: color-mix(in srgb, var(--accent) 4%, var(--bg-primary));
  color: var(--accent);
  font-size: 11px;
}
.branch-origin > svg { flex-shrink: 0; }
.branch-origin-copy { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.branch-origin-copy > span:first-child { color: var(--text-secondary); }
.branch-origin button { display: flex; align-items: center; gap: 4px; min-width: 0; color: var(--accent); text-align: left; overflow-wrap: anywhere; }
.branch-origin button:hover { text-decoration: underline; }
.branch-missing { color: var(--text-tertiary); overflow-wrap: anywhere; }
</style>
