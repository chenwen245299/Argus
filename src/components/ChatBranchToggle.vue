<script setup lang="ts">
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
defineProps<{ collapsed: boolean; title: string }>()
defineEmits<{ toggle: [] }>()
const { t } = useI18n()
</script>

<template>
  <button
    class="chat-branch-toggle"
    :aria-expanded="!collapsed"
    :aria-label="t(collapsed ? 'chat.expandBranches' : 'chat.collapseBranches', { title })"
    :title="t(collapsed ? 'chat.expandBranches' : 'chat.collapseBranches', { title })"
    @click.stop="$emit('toggle')"
  >
    <Icon :icon="collapsed ? 'fluent:chevron-right-16-regular' : 'fluent:chevron-down-16-regular'" width="13" height="13" />
  </button>
</template>

<style scoped>
.chat-branch-toggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 20px;
  height: 20px;
  color: var(--text-tertiary);
  border-radius: var(--radius-sm);
}
.chat-branch-toggle:hover { color: var(--accent); background: var(--bg-active); }
.chat-branch-toggle:focus-visible { outline: 2px solid var(--accent); outline-offset: -1px; }
</style>
