<script setup lang="ts">
/**
 * The AI 随航 settings section.
 *
 * Everything the AI does with the library lives here as a sub-tab: the two ways
 * a question gets answered (agent mode, where the model calls tools itself, and
 * RAG, where chunks are retrieved for it), the per-task analysis models, and the
 * arXiv crawler. They used to be four separate top-level sections, which made
 * settings read as a list of unrelated features rather than one place where the
 * AI is configured.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import AgentSettings from './AgentSettings.vue'
import RagSettings from './RagSettings.vue'
import ExtractionSettings from './ExtractionSettings.vue'
import ArxivSettings from './ArxivSettings.vue'

const props = defineProps<{ initialTab?: string }>()
const { t } = useI18n()

type Tab = 'agent' | 'rag' | 'extraction' | 'arxiv'
const TABS: Tab[] = ['agent', 'rag', 'extraction', 'arxiv']

const tabs: { id: Tab; label: string }[] = [
  { id: 'agent', label: 'qaSettings.agentTab' },
  { id: 'rag', label: 'qaSettings.ragTab' },
  { id: 'extraction', label: 'qaSettings.extractionTab' },
  { id: 'arxiv', label: 'qaSettings.arxivTab' },
]

function asTab(value: string | undefined): Tab | null {
  return TABS.includes(value as Tab) ? (value as Tab) : null
}

const activeTab = ref<Tab>(asTab(props.initialTab) ?? 'agent')

// Opening settings straight at a tab (the chat's "configure RAG" prompt, an
// arXiv or analysis entry point) has to land there even when this component is
// already mounted.
watch(() => props.initialTab, (tab) => {
  const next = asTab(tab)
  if (next) activeTab.value = next
})

const DESCRIPTIONS: Record<Tab, string> = {
  agent: 'agentSettings.desc',
  rag: 'ragSettings.desc',
  extraction: 'settings.extractionDesc',
  arxiv: 'arxivSettings.desc',
}
const description = computed(() => t(DESCRIPTIONS[activeTab.value]))
</script>

<template>
  <div class="qa-settings">
    <div class="qa-heading">
      <h2 class="qa-title">{{ t('settings.agent') }}</h2>
      <p class="qa-desc">{{ description }}</p>
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

    <div class="qa-panel" :class="{ 'qa-panel-flush': activeTab === 'extraction' || activeTab === 'arxiv' }">
      <AgentSettings v-if="activeTab === 'agent'" />
      <RagSettings v-else-if="activeTab === 'rag'" />
      <ExtractionSettings v-else-if="activeTab === 'extraction'" />
      <ArxivSettings v-else />
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
/* Those two panels were written as full settings pages with their own padding;
   the heading above already provides it, so pull it back. */
.qa-panel-flush :deep(.settings-panel),
.qa-panel-flush :deep(.settings-body) { padding: 0; }
</style>
