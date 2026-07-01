<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { updateStore } from '../stores/update'
import GeneralSettings from './settings/GeneralSettings.vue'
import AiSettings from './settings/AiSettings.vue'
import ExtractionSettings from './settings/ExtractionSettings.vue'
import AboutSettings from './settings/AboutSettings.vue'
import RagSettings from './settings/RagSettings.vue'
import ArxivSettings from './settings/ArxivSettings.vue'
import CanvasSettings from './settings/CanvasSettings.vue'

const props = defineProps<{ initialSection?: string }>()
const { t } = useI18n()
const emit = defineEmits<{ close: [] }>()

const tlHover = ref(false)

type Section = 'general' | 'ai' | 'extraction' | 'about' | 'arxiv' | 'rag' | 'canvas'

const activeSection = ref<Section>((props.initialSection as Section) ?? 'general')

const sections: { id: Section; label: string; placeholder?: boolean }[] = [
  { id: 'general', label: 'settings.general' },
  { id: 'ai', label: 'settings.ai' },
  { id: 'extraction', label: 'settings.extraction' },
  { id: 'arxiv', label: 'settings.arxiv' },
  { id: 'rag', label: 'settings.rag' },
  { id: 'canvas', label: 'settings.canvas' },
  { id: 'about', label: 'settings.about' },
]

function close() { emit('close') }

function onOverlayClick(e: MouseEvent) {
  if ((e.target as HTMLElement).classList.contains('modal-overlay')) {
    close()
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onUnmounted(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <Teleport to="body">
    <div class="modal-overlay" @click="onOverlayClick">
      <Transition name="modal" appear>
        <div class="modal">
          <!-- Header: macOS traffic lights -->
          <div class="modal-header">
            <div class="traffic-lights" @mouseenter="tlHover = true" @mouseleave="tlHover = false">
              <button class="tl-btn tl-red" @click="close" :title="t('settings.close')">
                <svg v-if="tlHover" width="6" height="6" viewBox="0 0 10 10" fill="none" stroke="#4d0000" stroke-width="1.8" stroke-linecap="round">
                  <line x1="2" y1="2" x2="8" y2="8"/><line x1="8" y1="2" x2="2" y2="8"/>
                </svg>
              </button>
              <button class="tl-btn tl-yellow">
                <svg v-if="tlHover" width="6" height="6" viewBox="0 0 10 2" fill="none" stroke="#5c3d00" stroke-width="1.8" stroke-linecap="round">
                  <line x1="1" y1="1" x2="9" y2="1"/>
                </svg>
              </button>
              <button class="tl-btn tl-green">
                <svg v-if="tlHover" width="7" height="7" viewBox="0 0 10 10" fill="none" stroke="#003300" stroke-width="1.5" stroke-linecap="round">
                  <path d="M2 5 L4.5 7.5 L8.5 2.5"/>
                </svg>
              </button>
            </div>
            <span class="modal-title">{{ t('settings.title') }}</span>
            <div class="tl-spacer" />
          </div>

          <!-- Body: nav + content -->
          <div class="modal-body">
            <!-- Left nav -->
            <nav class="settings-nav">
              <button
                v-for="sec in sections"
                :key="sec.id"
                class="nav-item"
                :class="{ active: activeSection === sec.id, placeholder: sec.placeholder }"
                @click="!sec.placeholder && (activeSection = sec.id)"
              >
                {{ t(sec.label) }}
                <span v-if="sec.placeholder" class="coming-badge">{{ t('settings.comingSoon') }}</span>
                <span v-else-if="sec.id === 'about' && updateStore.hasUpdate && activeSection !== 'about'" class="update-nav-dot" />
              </button>
            </nav>

            <!-- Right content -->
            <div class="settings-content">
              <GeneralSettings v-if="activeSection === 'general'" />
              <AiSettings v-else-if="activeSection === 'ai'" />
              <RagSettings v-else-if="activeSection === 'rag'" />
              <ArxivSettings v-else-if="activeSection === 'arxiv'" />
              <ExtractionSettings v-else-if="activeSection === 'extraction'" />
              <CanvasSettings v-else-if="activeSection === 'canvas'" />
              <AboutSettings v-else-if="activeSection === 'about'" />
              <div v-else class="placeholder-panel">
                <div class="placeholder-inner">
                  <span class="coming-badge-lg">{{ t('settings.comingSoon') }}</span>
                  <p>{{ t(sections.find(s => s.id === activeSection)?.label ?? '') }}</p>
                </div>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </div>
  </Teleport>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.34);
  backdrop-filter: blur(10px) saturate(1.15);
  -webkit-backdrop-filter: blur(10px) saturate(1.15);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.modal {
  background: var(--bg-primary);
  border-radius: 16px;
  box-shadow: var(--shadow-lg);
  border: 1px solid var(--border-subtle);
  width: min(1120px, 94vw);
  height: min(720px, 88vh);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  font-family: var(--font-sans);
  font-size: var(--font-size-base);
  line-height: 1.45;
  letter-spacing: 0;
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
  user-select: none;
  -webkit-user-select: none;
}

.modal :deep(input),
.modal :deep(textarea),
.modal :deep(select),
.modal :deep([contenteditable='true']) {
  user-select: text;
  -webkit-user-select: text;
}

.modal-header {
  display: flex;
  align-items: center;
  height: 44px;
  padding: 0 18px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary));
  gap: 0;
}

/* macOS traffic lights */
.traffic-lights {
  display: flex;
  align-items: center;
  gap: 7px;
  flex-shrink: 0;
}
.tl-btn {
  width: 13px;
  height: 13px;
  border-radius: 50%;
  border: none;
  cursor: default;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: filter 0.1s;
  padding: 0;
  flex-shrink: 0;
}
.tl-red    { background: #FF5F57; cursor: pointer; }
.tl-yellow { background: #FFBD2E; }
.tl-green  { background: #28C840; }
.tl-btn:active { filter: brightness(0.85); }

.modal-title {
  flex: 1;
  text-align: center;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  letter-spacing: 0;
}

.tl-spacer { width: 46px; flex-shrink: 0; }

.modal-body {
  display: flex;
  flex: 1;
  overflow: hidden;
}

/* Left nav — macOS preferences sidebar style */
.settings-nav {
  width: 186px;
  flex-shrink: 0;
  background: color-mix(in srgb, var(--bg-secondary) 78%, var(--bg-primary));
  border-right: 1px solid var(--border-subtle);
  padding: 14px 10px;
  overflow-y: auto;
}
.nav-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  margin: 1px 0;
  padding: 8px 12px;
  font-size: 13px;
  font-weight: 500;
  line-height: 1.35;
  color: var(--text-secondary);
  text-align: left;
  border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s;
  gap: 6px;
}
.nav-item:hover:not(.placeholder) { background: var(--bg-hover); color: var(--text-primary); }
.nav-item.active {
  background: var(--accent-light);
  color: var(--accent);
  font-weight: 600;
}
.nav-item.placeholder { cursor: default; opacity: 0.6; }

.nav-item.active .coming-badge {
  background: rgba(255,255,255,0.20);
  color: rgba(255,255,255,0.85);
}
.coming-badge {
  font-size: 9px;
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  white-space: nowrap;
}
.update-nav-dot {
  width: 7px; height: 7px;
  border-radius: 50%;
  background: #ef4444;
  flex-shrink: 0;
  margin-left: auto;
  box-shadow: 0 0 0 2px rgba(239, 68, 68, 0.25);
}

/* Right content */
.settings-content {
  flex: 1;
  overflow-y: auto;
  min-width: 0;
  position: relative;
  display: flex;
  flex-direction: column;
  background: color-mix(in srgb, var(--bg-primary) 96%, var(--bg-secondary));
  color: var(--text-primary);
}

.settings-content :deep(*) {
  letter-spacing: 0;
}

.settings-content :deep(h2),
.settings-content :deep(.section-title),
.settings-content :deep(.detail-title) {
  font-weight: 650;
  line-height: 1.25;
  color: var(--text-primary);
}

.settings-content :deep(.setting-label),
.settings-content :deep(.field-label),
.settings-content :deep(.list-title),
.settings-content :deep(.pane-title),
.settings-content :deep(.section-sep) {
  text-transform: none;
  letter-spacing: 0;
}

.placeholder-panel {
  display: flex; align-items: center; justify-content: center;
  height: 100%; color: var(--text-tertiary);
}
.placeholder-inner { text-align: center; }
.coming-badge-lg {
  display: inline-block;
  font-size: var(--font-size-xs);
  background: var(--bg-tertiary); color: var(--text-tertiary);
  padding: 3px 12px; border-radius: var(--radius-pill);
  margin-bottom: 12px;
}
.placeholder-inner p { font-size: var(--font-size-sm); color: var(--text-tertiary); }

/* Transition */
.modal-enter-active { animation: modal-in 0.18s cubic-bezier(0.34, 1.56, 0.64, 1); }
.modal-leave-active { animation: modal-in 0.14s ease reverse; }
@keyframes modal-in {
  from { opacity: 0; transform: scale(0.94) translateY(10px); }
  to   { opacity: 1; transform: scale(1) translateY(0); }
}
</style>
