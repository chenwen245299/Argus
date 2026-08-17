<script setup lang="ts">
/**
 * The 智能问答 settings section.
 *
 * Both ways the app answers a question about the library live here: agent mode,
 * where the model calls tools itself, and RAG, where chunks are retrieved and
 * handed to it. They were separate top-level sections, which made them look
 * like unrelated features rather than two settings for the same thing.
 */
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import AgentSettings from './AgentSettings.vue'
import RagSettings from './RagSettings.vue'

const props = defineProps<{ initialTab?: string }>()
const { t } = useI18n()

type Tab = 'agent' | 'rag'

const tabs: { id: Tab; label: string }[] = [
  { id: 'agent', label: 'qaSettings.agentTab' },
  { id: 'rag', label: 'qaSettings.ragTab' },
]

const activeTab = ref<Tab>(props.initialTab === 'rag' ? 'rag' : 'agent')

// Opening settings straight at RAG (from the chat's "configure RAG" prompts)
// has to land on the RAG tab even when this component is already mounted.
watch(() => props.initialTab, (tab) => {
  if (tab === 'rag' || tab === 'agent') activeTab.value = tab
})
</script>

<template>
  <div class="qa-settings">
    <div class="qa-heading">
      <h2 class="qa-title">{{ t('settings.agent') }}</h2>
      <p class="qa-desc">
        {{ activeTab === 'agent' ? t('agentSettings.desc') : t('ragSettings.desc') }}
      </p>
    </div>

    <div class="qa-tabs">
      <button
        v-for="tab in tabs"
        :key="tab.id"
        class="qa-tab"
        :class="{ active: activeTab === tab.id }"
        @click="activeTab = tab.id"
      >
        {{ t(tab.label) }}
      </button>
    </div>

    <div class="qa-panel">
      <AgentSettings v-if="activeTab === 'agent'" />
      <RagSettings v-else />
    </div>
  </div>
</template>

<style scoped>
.qa-settings {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 24px 28px;
  box-sizing: border-box;
  height: 100%;
  overflow: hidden;
}

.qa-heading { display: flex; flex-direction: column; gap: 6px; }
.qa-title { font-size: 18px; font-weight: 600; color: var(--text-primary); margin: 0; }
.qa-desc { font-size: 13px; color: var(--text-secondary); margin: 0; line-height: 1.5; }

.qa-tabs {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}
.qa-tab {
  padding: 5px 13px;
  font-size: 12.5px;
  font-weight: 500;
  color: var(--text-secondary);
  border-radius: var(--radius-pill);
}
.qa-tab:hover { background: var(--bg-hover); }
.qa-tab.active { background: var(--accent); color: #fff; }

/* The panels are the scrolling region, not the page, so the heading and tabs
   stay put while a long form scrolls under them. */
.qa-panel {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}
</style>
