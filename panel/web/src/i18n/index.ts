import { useStorage } from '@vueuse/core'
import { watch } from 'vue'
import { createI18n } from 'vue-i18n'
import { LOCALES, messages, type Locale } from './messages'

export { LOCALES, type Locale }

function preferredLocale(): Locale {
  const languages = typeof navigator === 'undefined' ? [] : navigator.languages
  return languages.some((language) => language.toLowerCase().startsWith('zh')) ? 'zh-CN' : 'en'
}

const stored = useStorage<Locale>('pingora-panel.locale', preferredLocale())
if (!LOCALES.includes(stored.value)) {
  stored.value = preferredLocale()
}

const time = { hour: '2-digit', minute: '2-digit', second: '2-digit' } as const

export const i18n = createI18n({
  legacy: false,
  locale: stored.value,
  fallbackLocale: 'en',
  messages,
  datetimeFormats: { 'zh-CN': { time }, en: { time } },
})

watch(
  () => i18n.global.locale.value,
  (locale) => {
    stored.value = locale
    document.documentElement.lang = locale
  },
  { immediate: true },
)
