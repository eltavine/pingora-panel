import en from './locales/en'
import zhCN from './locales/zh-CN'

export type { Locale } from './locales'

/** Every language's messages at once, for checks that compare them. */
export const messages = { 'zh-CN': zhCN, en } as const
