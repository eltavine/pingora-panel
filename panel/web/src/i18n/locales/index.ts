import type { Messages } from './zh-CN'

export type { Messages }

export const LOCALES = ['zh-CN', 'en'] as const
export type Locale = (typeof LOCALES)[number]

/** Each language's messages, fetched the first time the language is used. */
export const loadMessages: Record<Locale, () => Promise<Messages>> = {
  'zh-CN': async () => (await import('./zh-CN')).default,
  en: async () => (await import('./en')).default,
}
