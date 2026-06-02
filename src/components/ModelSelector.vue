<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useAiStore } from '../stores/ai'
import type { ModelSelection } from '../types'

const props = defineProps<{
  modelValue: ModelSelection | null
  placeholder?: string
}>()

const emit = defineEmits<{
  'update:modelValue': [v: ModelSelection | null]
}>()

const { t } = useI18n()
const ai = useAiStore()

const groups = computed(() => ai.groupedModels)

const selectValue = computed({
  get: () => {
    if (!props.modelValue) return ''
    return `${props.modelValue.providerId}::${props.modelValue.modelId}`
  },
  set: (val: string) => {
    if (!val) {
      emit('update:modelValue', null)
      return
    }
    const sep = val.indexOf('::')
    if (sep === -1) return
    emit('update:modelValue', {
      providerId: val.slice(0, sep),
      modelId: val.slice(sep + 2),
    })
  },
})
</script>

<template>
  <select v-model="selectValue" class="model-select">
    <option value="">{{ placeholder ?? t('copilot.noModel') }}</option>
    <optgroup v-for="group in groups" :key="group.id" :label="group.name">
      <option
        v-for="m in group.models"
        :key="m.modelId"
        :value="`${m.providerId}::${m.modelId}`"
      >{{ m.displayName }}</option>
    </optgroup>
  </select>
</template>

<style scoped>
.model-select {
  font-size: var(--font-size-xs);
  padding: 3px 6px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-secondary);
  color: var(--text-secondary);
  cursor: pointer;
  max-width: 200px;
}
.model-select:focus { outline: none; border-color: var(--accent); }
</style>
