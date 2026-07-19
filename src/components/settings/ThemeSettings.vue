<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useSettingsStore } from '../../stores/settings'
import { Icon } from '@iconify/vue'
import { THEMES, type ThemeMeta } from '../../utils/themes'
import type { ConcreteThemeId } from '../../types'
import ThemeCard from './ThemeCard.vue'

const { t, locale } = useI18n()
const settingsStore = useSettingsStore()

onMounted(() => { settingsStore.load() })

function themeName(th: ThemeMeta) {
  return locale.value === 'en' ? th.nameEn : th.nameZh
}

const lightThemes = computed(() => THEMES.filter(th => th.kind === 'light'))
const darkThemes = computed(() => THEMES.filter(th => th.kind === 'dark'))

// ── Top-level choice ──────────────────────────────────────────────────
// Either a single fixed palette, or follow the OS light/dark setting. This is
// the one decision that used to be tangled up with picking a theme; now it is
// its own explicit switch.
const mode = computed<'fixed' | 'system'>(() =>
  settingsStore.settings.appearance === 'system' ? 'system' : 'fixed',
)

function setMode(next: 'fixed' | 'system') {
  if (next === mode.value) return
  if (next === 'system') {
    settingsStore.save({ appearance: 'system' })
  } else {
    // Land on whatever palette is on screen right now so flipping to fixed
    // doesn't jump the UI. data-theme is always concrete (never 'system').
    const active = document.documentElement.getAttribute('data-theme') as ConcreteThemeId | null
    settingsStore.save({ appearance: active ?? settingsStore.settings.appearance_light ?? 'light' })
  }
}

// ── Fixed mode: pick one palette ──────────────────────────────────────
type Filter = 'all' | 'light' | 'dark'
const filter = ref<Filter>('all')
const filterOptions: { id: Filter; labelKey: string }[] = [
  { id: 'all', labelKey: 'settings.themeFilterAll' },
  { id: 'light', labelKey: 'settings.themeFilterLight' },
  { id: 'dark', labelKey: 'settings.themeFilterDark' },
]
const visibleThemes = computed(() =>
  THEMES.filter(th => filter.value === 'all' || th.kind === filter.value),
)
function selectFixed(id: ConcreteThemeId) {
  settingsStore.save({ appearance: id })
}

// ── System mode: a palette per OS appearance ──────────────────────────
const lightPref = computed({
  get: () => settingsStore.settings.appearance_light ?? 'light',
  set: (id: ConcreteThemeId) => settingsStore.save({ appearance_light: id }),
})
const darkPref = computed({
  get: () => settingsStore.settings.appearance_dark ?? 'dark',
  set: (id: ConcreteThemeId) => settingsStore.save({ appearance_dark: id }),
})
</script>

<template>
  <div class="settings-panel theme-market">
    <h2>{{ t('settings.themes') }}</h2>
    <p class="market-desc">{{ t('settings.themesDesc') }}</p>

    <!-- Primary choice: a fixed palette, or follow the system light/dark -->
    <div class="mode-toggle">
      <button
        class="mode-card"
        :class="{ active: mode === 'fixed' }"
        @click="setMode('fixed')"
      >
        <span class="mode-swatch fixed" />
        <span class="mode-text">
          <span class="mode-title">{{ t('settings.themeModeFixed') }}</span>
          <span class="mode-desc">{{ t('settings.themeModeFixedDesc') }}</span>
        </span>
        <Icon v-if="mode === 'fixed'" icon="fluent:checkmark-circle-24-filled" class="mode-check" width="18" height="18" />
      </button>
      <button
        class="mode-card"
        :class="{ active: mode === 'system' }"
        @click="setMode('system')"
      >
        <span class="mode-swatch system" />
        <span class="mode-text">
          <span class="mode-title">{{ t('settings.themeSystem') }}</span>
          <span class="mode-desc">{{ t('settings.themeModeSystemDesc') }}</span>
        </span>
        <Icon v-if="mode === 'system'" icon="fluent:checkmark-circle-24-filled" class="mode-check" width="18" height="18" />
      </button>
    </div>

    <!-- FIXED: browse and pick a single palette -->
    <template v-if="mode === 'fixed'">
      <div class="segmented-control market-filter">
        <button
          v-for="opt in filterOptions"
          :key="opt.id"
          class="segment-btn"
          :class="{ active: filter === opt.id }"
          @click="filter = opt.id"
        >
          {{ t(opt.labelKey) }}
        </button>
      </div>

      <div class="market-grid">
        <ThemeCard
          v-for="(th, i) in visibleThemes"
          :key="th.id"
          :theme="th"
          :name="themeName(th)"
          :active="settingsStore.settings.appearance === th.id"
          :index="i"
          @select="selectFixed(th.id)"
        />
      </div>
    </template>

    <!-- SYSTEM: pick a palette for each OS appearance -->
    <template v-else>
      <section class="mode-section">
        <div class="section-head">
          <span class="section-dot light" />
          <span class="section-title">{{ t('settings.themeWhenLight') }}</span>
          <span class="section-sub">{{ t('settings.themeWhenLightDesc') }}</span>
        </div>
        <div class="market-grid">
          <ThemeCard
            v-for="(th, i) in lightThemes"
            :key="th.id"
            :theme="th"
            :name="themeName(th)"
            :active="lightPref === th.id"
            :index="i"
            @select="lightPref = th.id"
          />
        </div>
      </section>

      <section class="mode-section">
        <div class="section-head">
          <span class="section-dot dark" />
          <span class="section-title">{{ t('settings.themeWhenDark') }}</span>
          <span class="section-sub">{{ t('settings.themeWhenDarkDesc') }}</span>
        </div>
        <div class="market-grid">
          <ThemeCard
            v-for="(th, i) in darkThemes"
            :key="th.id"
            :theme="th"
            :name="themeName(th)"
            :active="darkPref === th.id"
            :index="i"
            @select="darkPref = th.id"
          />
        </div>
      </section>
    </template>
  </div>
</template>

<style scoped>
.settings-panel { padding: 24px 28px; }
h2 { font-size: 18px; font-weight: 600; margin-bottom: 6px; color: var(--text-primary); }

.market-desc {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  margin: 0 0 18px;
  line-height: 1.5;
}

/* ── Primary mode toggle ── */
.mode-toggle {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
  margin-bottom: 22px;
}
.mode-card {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 13px 15px;
  border: 2px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: var(--bg-secondary);
  cursor: pointer;
  text-align: left;
  transition: border-color 0.15s, box-shadow 0.15s, transform 0.15s;
}
.mode-card:hover {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light), var(--shadow-sm);
  transform: translateY(-1px);
}
.mode-card.active {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light);
}

.mode-swatch {
  width: 34px;
  height: 34px;
  border-radius: var(--radius-md);
  flex-shrink: 0;
  border: 1px solid rgba(128, 128, 128, 0.18);
}
.mode-swatch.fixed { background: var(--accent); }
.mode-swatch.system { background: linear-gradient(135deg, #ffffff 50%, #1c1c1e 50%); }

.mode-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}
.mode-title {
  font-size: var(--font-size-base);
  font-weight: 600;
  color: var(--text-primary);
}
.mode-card.active .mode-title { color: var(--accent); }
.mode-desc {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  line-height: 1.4;
}
.mode-check {
  color: var(--accent);
  flex-shrink: 0;
  margin-left: auto;
}

/* ── Fixed-mode filter ── */
.market-filter { margin-bottom: 18px; }

.segmented-control {
  display: inline-flex;
  gap: 2px;
  padding: 3px;
  background: var(--bg-tertiary);
  border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle);
}
.segment-btn {
  min-width: 72px;
  padding: 5px 14px;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
  transition: background 0.12s, color 0.12s, box-shadow 0.12s;
}
.segment-btn:hover { color: var(--text-primary); }
.segment-btn.active {
  background: var(--bg-primary);
  color: var(--text-primary);
  box-shadow: var(--shadow-sm);
}

/* ── Card grid ── */
.market-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 14px;
  padding-bottom: 2px;
}

/* ── System-mode sections ── */
.mode-section { margin-bottom: 24px; }
.mode-section:last-child { margin-bottom: 6px; }

.section-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 12px;
}
.section-dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  flex-shrink: 0;
  align-self: center;
  border: 1px solid rgba(128, 128, 128, 0.25);
}
.section-dot.light { background: #ffffff; }
.section-dot.dark { background: #1c1c1e; }
.section-title {
  font-size: var(--font-size-base);
  font-weight: 600;
  color: var(--text-primary);
}
.section-sub {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
</style>
