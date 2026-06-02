<script setup lang="ts">
import AiTab from './AiTab.vue'

const props = defineProps<{ slug?: string | null }>()
const emit = defineEmits<{ 'open-settings': [] }>()
</script>

<template>
  <AiTab
    :slug="props.slug ?? null"
    class="analysis-tab"
    @open-settings="emit('open-settings')"
  />
</template>

<style scoped>
.analysis-tab {
  height: 100%;
  overflow: hidden;
}
</style>
