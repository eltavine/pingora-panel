import en from './locales/en'
import zhCN from './locales/zh-CN'

export type { Locale } from './locales'

/** Messages by key, nested by the dots of their keys. */
export type Tree = { [key: string]: string | Tree }

/** Every feature's own messages, by file. */
export const featureMessages = import.meta.glob<{ default: object }>('../features/*/locales/*.ts', {
  eager: true,
})

function merged(base: Tree, extra: Tree): Tree {
  const tree: Tree = { ...base }
  for (const [key, value] of Object.entries(extra)) {
    const current = tree[key]
    tree[key] =
      typeof value === 'object' && typeof current === 'object' ? merged(current, value) : value
  }
  return tree
}

function withFeatures(locale: string, base: object): Tree {
  return Object.entries(featureMessages)
    .filter(([path]) => path.endsWith(`/${locale}.ts`))
    .reduce((tree, [, module]) => merged(tree, module.default as Tree), base as Tree)
}

/** Every language's messages at once, the features' included, for checks that compare them. */
export const messages = {
  'zh-CN': withFeatures('zh-CN', zhCN),
  en: withFeatures('en', en),
} as const
