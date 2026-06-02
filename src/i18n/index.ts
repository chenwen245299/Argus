import { createI18n } from 'vue-i18n'
import { zh, en } from './locales'

export type Locale = 'zh' | 'en'
const STORAGE_KEY = 'argus_locale'

const saved = localStorage.getItem(STORAGE_KEY) as Locale | null
const defaultLocale: Locale = saved ?? 'zh'

export const i18n = createI18n({
  legacy: false,          // Composition API mode
  locale: defaultLocale,
  fallbackLocale: 'zh',
  messages: { zh, en },
})

export function toggleLocale() {
  const next: Locale = i18n.global.locale.value === 'zh' ? 'en' : 'zh'
  setLocale(next)
}

export function setLocale(locale: Locale) {
  i18n.global.locale.value = locale
  localStorage.setItem(STORAGE_KEY, locale)
}

export function currentLangLabel(): string {
  return i18n.global.locale.value === 'zh' ? 'EN' : '中文'
}
