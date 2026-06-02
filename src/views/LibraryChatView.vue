<script setup lang="ts">
import { ref } from 'vue'
import LibraryChat from '../components/LibraryChat.vue'
import SettingsModal from '../components/SettingsModal.vue'

const showSettings = ref(false)
const settingsSection = ref('ai')

function openSettings(section: 'ai' | 'rag' = 'ai') {
  settingsSection.value = section
  showSettings.value = true
}
</script>

<template>
  <div class="library-chat-view">
    <LibraryChat @open-settings="openSettings" />
    <SettingsModal
      v-if="showSettings"
      :initial-section="settingsSection"
      @close="showSettings = false"
    />
  </div>
</template>

<style scoped>
.library-chat-view {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font-family: var(--font-sans);
}
</style>
