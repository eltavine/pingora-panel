import { useStorage } from '@vueuse/core'
import { watch } from 'vue'
import { createI18n } from 'vue-i18n'
import { LOCALES, loadMessages, type Locale, type Messages } from './locales'

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
const datetime = {
  year: 'numeric',
  month: '2-digit',
  day: '2-digit',
  hour: '2-digit',
  minute: '2-digit',
} as const
/** Log records: to the millisecond. */
const precise = { ...datetime, second: '2-digit', fractionalSecondDigits: 3 } as const

export const i18n = createI18n({
  legacy: false,
  locale: stored.value,
  messages: {} as Record<Locale, Messages>,
  datetimeFormats: {
    'zh-CN': { time, datetime, precise },
    en: { time, datetime, precise },
  },
})

watch(
  () => i18n.global.locale.value,
  (locale) => {
    stored.value = locale
    document.documentElement.lang = locale
  },
  { immediate: true },
)

/** Switches the console to `locale`, fetching its messages the first time. */
export async function setLocale(locale: Locale): Promise<void> {
  if (!i18n.global.availableLocales.includes(locale)) {
    i18n.global.setLocaleMessage(locale, await loadMessages[locale]())
  }
  i18n.global.locale.value = locale
}
