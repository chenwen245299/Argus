<script setup lang="ts">
/**
 * What OpenRouter's server tools contributed to one answer: the pages a search
 * consulted, and any image the model drew.
 *
 * Shown under the answer rather than inline, because the model has already
 * written its own prose around these — this is the receipt, not the content.
 * Renders nothing when no tool ran, which is the common case.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import type { ServerToolTrace } from '../types'

const props = defineProps<{ trace?: ServerToolTrace | null }>()

const { t } = useI18n()

const citations = computed(() => props.trace?.citations ?? [])
const images = computed(() => props.trace?.images ?? [])

/** `{ web_search: 2 }` becomes "联网搜索 x2", in a stable order. */
const calls = computed(() =>
  Object.entries(props.trace?.calls ?? {})
    .filter(([, n]) => n > 0)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([tool, n]) => ({ tool, n, label: toolLabel(tool) }))
)

const hasAnything = computed(
  () => citations.value.length > 0 || images.value.length > 0 || calls.value.length > 0
)

const TOOL_LABELS: Record<string, string> = {
  web_search: 'aiService.otTool_web_search',
  web_fetch: 'aiService.otTool_web_fetch',
  datetime: 'aiService.otTool_datetime',
  image_generation: 'aiService.otTool_image_generation',
}

function toolLabel(tool: string) {
  const key = TOOL_LABELS[tool]
  return key ? t(key) : tool.replace(/_/g, ' ')
}

/** The host, which is what identifies a source at a glance. */
function hostOf(url: string) {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return url
  }
}
</script>

<template>
  <div v-if="hasAnything" class="st-trace">
    <div v-if="calls.length" class="st-calls">
      <Icon icon="fluent:flash-24-regular" width="12" height="12" />
      <span v-for="c in calls" :key="c.tool" class="st-call">
        {{ c.label }}<span v-if="c.n > 1" class="st-call-n">×{{ c.n }}</span>
      </span>
    </div>

    <div v-if="images.length" class="st-images">
      <a
        v-for="(src, i) in images"
        :key="i"
        :href="src"
        target="_blank"
        rel="noopener"
        class="st-image"
        :title="t('serverTools.openImage')"
      >
        <img :src="src" alt="" />
      </a>
    </div>

    <div v-if="citations.length" class="st-citations">
      <a
        v-for="(c, i) in citations"
        :key="c.url"
        :href="c.url"
        target="_blank"
        rel="noopener"
        class="st-citation"
        :title="c.title ? `${c.title}\n${c.url}` : c.url"
      >
        <span class="st-citation-index">{{ i + 1 }}</span>
        <span class="st-citation-text">{{ c.title || hostOf(c.url) }}</span>
        <!-- Only when it adds something: an untitled citation already shows its
             host as the label, and repeating it reads like a rendering bug. -->
        <span v-if="c.title" class="st-citation-host">{{ hostOf(c.url) }}</span>
      </a>
    </div>
  </div>
</template>

<style scoped>
.st-trace {
  display: flex;
  flex-direction: column;
  gap: 7px;
  margin-top: 8px;
}

.st-calls {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  color: var(--text-tertiary);
  font-size: 10px;
}
.st-call {
  padding: 1px 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-tertiary) 12%, transparent);
}
.st-call-n {
  margin-left: 3px;
  font-variant-numeric: tabular-nums;
  opacity: 0.75;
}

.st-images {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.st-image {
  display: block;
  max-width: 240px;
  border-radius: 10px;
  overflow: hidden;
  border: 1px solid var(--border-default);
  line-height: 0;
}
.st-image img {
  width: 100%;
  height: auto;
  display: block;
}

.st-citations {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.st-citation {
  display: flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  padding: 3px 6px;
  border-radius: 6px;
  color: var(--text-secondary);
  font-size: 11px;
  text-decoration: none;
}
.st-citation:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.st-citation-index {
  flex-shrink: 0;
  min-width: 14px;
  color: var(--text-tertiary);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
}
.st-citation-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.st-citation-host {
  flex-shrink: 0;
  margin-left: auto;
  color: var(--text-tertiary);
  font-size: 10px;
}
</style>
