<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import GuidedReadingIcon from './GuidedReadingIcon.vue'
import { useGuidedReadingStore } from '../stores/guidedReading'
import { guidedReadingPrompt } from '../utils/guidedReading'
import { BUILTIN_GUIDED_READING_ID, type GuidedReadingPreset } from '../types/guidedReading'

const props = defineProps<{ disabled: boolean; compact: boolean }>()
const emit = defineEmits<{ start: [preset: GuidedReadingPreset] }>()
const { t, locale } = useI18n()
const store = useGuidedReadingStore()
const trigger = ref<HTMLButtonElement | null>(null), menu = ref<HTMLElement | null>(null)
const open = ref(false), busy = ref(false), error = ref('')
const position = ref({ left: '0px', top: '0px' })
let disposed = false
const choices = computed<GuidedReadingPreset[]>(() => [{ id: BUILTIN_GUIDED_READING_ID,
  name: t('guidedReadingSettings.builtin'), prompt: guidedReadingPrompt(locale.value) }, ...store.settings.presets])

async function launch(id?: string) {
  if (props.disabled || busy.value) return
  busy.value = true; error.value = ''; open.value = false
  try {
    if (!await store.load() || disposed || props.disabled) return
    const preset = choices.value.find(p => p.id === (id ?? store.settings.defaultId))
    if (!preset) { error.value = t('guidedReadingSettings.loadFailed'); return }
    emit('start', { ...preset })
  } catch { if (!disposed) error.value = t('guidedReadingSettings.loadFailed') }
  finally { if (!disposed) busy.value = false }
}
async function showMenu(event: MouseEvent | KeyboardEvent) {
  event.preventDefault()
  if (props.disabled || busy.value) return
  busy.value = true; error.value = ''
  try {
    if (!await store.load() || disposed || props.disabled) return
    open.value = true
    await nextTick()
    const anchor = trigger.value?.getBoundingClientRect(), bounds = menu.value?.getBoundingClientRect()
    if (!anchor || !bounds) return
    const x = event instanceof MouseEvent && event.clientX ? event.clientX : anchor.left
    const y = event instanceof MouseEvent && event.clientY ? event.clientY : anchor.bottom + 6
    position.value = { left: `${Math.max(8, Math.min(x, innerWidth - bounds.width - 8))}px`,
      top: `${Math.max(8, Math.min(y, innerHeight - bounds.height - 8))}px` }
    menu.value?.querySelector<HTMLButtonElement>('button')?.focus()
  } catch { if (!disposed) error.value = t('guidedReadingSettings.loadFailed') }
  finally { if (!disposed) busy.value = false }
}
function onMenuKey(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); open.value = false; trigger.value?.focus() }
  else if (event.key === 'Tab') open.value = false
  else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
    event.preventDefault()
    const buttons = [...(menu.value?.querySelectorAll<HTMLButtonElement>('button') ?? [])]
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1
      : (current + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length
    buttons[index]?.focus()
  }
}
function outside(event: MouseEvent) {
  if (!trigger.value?.contains(event.target as Node) && !menu.value?.contains(event.target as Node)) {
    open.value = false; error.value = ''
  }
}
function close() { open.value = false }
watch(() => props.disabled, value => { if (value) close() })
onMounted(() => { document.addEventListener('mousedown', outside); window.addEventListener('resize', close) })
onUnmounted(() => { disposed = true; document.removeEventListener('mousedown', outside); window.removeEventListener('resize', close) })
</script>

<template>
  <button ref="trigger" class="guided-reading-btn" :class="{ compact }" :title="t('guidedReading.hint')"
    :aria-label="t('guidedReading.title')" aria-haspopup="menu" :aria-expanded="open" :disabled="disabled || busy"
    @click="launch()" @contextmenu="showMenu" @keydown="event => { if (event.key === 'ContextMenu' || event.key === 'F10' && event.shiftKey) showMenu(event) }">
    <GuidedReadingIcon /><span>{{ t('guidedReading.title') }}</span>
  </button>
  <Teleport to="body">
    <div v-if="open" ref="menu" class="guided-reading-menu" :style="position" role="menu"
      :aria-label="t('guidedReadingSettings.chooseOnce')" @keydown="onMenuKey" @contextmenu.prevent>
      <div class="menu-heading">{{ t('guidedReadingSettings.chooseOnce') }}</div>
      <button v-for="preset in choices" :key="preset.id" role="menuitem" @click="launch(preset.id)">
        <span>{{ preset.name }}</span><small v-if="store.settings.defaultId === preset.id">{{ t('guidedReadingSettings.defaultBadge') }}</small>
      </button>
    </div>
    <div v-if="error" class="guided-reading-error" role="alert">{{ error }}</div>
  </Teleport>
</template>

<style scoped>
.guided-reading-btn { display: inline-flex; align-items: center; justify-content: center; gap: 5px; height: 28px; padding: 0 8px; border: 1px solid color-mix(in srgb, var(--accent) 22%, transparent); border-radius: 8px; background: color-mix(in srgb, var(--accent) 9%, var(--bg-primary)); color: var(--accent); font-size: 12px; font-weight: 600; white-space: nowrap; cursor: pointer; }
.guided-reading-btn :deep(svg) { font-size: 21px; }
.guided-reading-btn:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 16%, var(--bg-primary)); border-color: color-mix(in srgb, var(--accent) 45%, transparent); }
.guided-reading-btn:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.guided-reading-btn:disabled { opacity: .45; cursor: default; }
.guided-reading-btn.compact { width: 30px; padding: 0; }
.guided-reading-btn.compact span { display: none; }
.guided-reading-menu { position: fixed; z-index: 10010; width: min(280px, calc(100vw - 16px)); max-height: min(400px, calc(100vh - 16px)); overflow-y: auto; padding: 5px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--bg-primary); box-shadow: var(--shadow-lg); color: var(--text-primary); }
.menu-heading { padding: 7px 9px; font-size: 11px; color: var(--text-tertiary); }
.guided-reading-menu button { display: flex; align-items: center; gap: 12px; width: 100%; padding: 9px; border-radius: var(--radius-sm); text-align: left; font-size: 12px; }
.guided-reading-menu button span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.guided-reading-menu button small { color: var(--text-tertiary); font-size: 10px; }
.guided-reading-menu button:hover, .guided-reading-menu button:focus-visible { background: var(--bg-hover); outline: none; }
.guided-reading-error { position: fixed; z-index: 10010; bottom: 24px; left: 50%; transform: translateX(-50%); max-width: calc(100vw - 32px); padding: 10px 14px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--bg-primary); color: var(--text-primary); box-shadow: var(--shadow-lg); font-size: 12px; }
</style>
