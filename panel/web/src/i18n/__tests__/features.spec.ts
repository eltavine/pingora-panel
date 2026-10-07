import { describe, expect, it } from 'vitest'
import en from '../locales/en'
import zhCN from '../locales/zh-CN'
import { featureMessages } from '../messages'

function* keys(tree: object, prefix = ''): Generator<string> {
  for (const [key, value] of Object.entries(tree)) {
    if (typeof value === 'string') {
      yield prefix + key
    } else {
      yield* keys(value as object, `${prefix}${key}.`)
    }
  }
}

const byFeature = Object.entries(featureMessages).reduce<Record<string, Record<string, object>>>(
  (features, [path, module]) => {
    const [, feature, locale] = /features\/([^/]+)\/locales\/([^/]+)\.ts$/.exec(path)!
    features[feature!] = { ...features[feature!], [locale!]: module.default }
    return features
  },
  {},
)

describe("features' own messages", () => {
  it('exist in every language with the same keys', () => {
    expect(Object.keys(byFeature).length).toBeGreaterThan(0)
    for (const [feature, locales] of Object.entries(byFeature)) {
      expect({ feature, en: [...keys(locales.en!)] }).toEqual({
        feature,
        en: [...keys(locales['zh-CN']!)],
      })
    }
  })

  it('leave the shared messages to the shared files', () => {
    const shared = new Set([...keys(zhCN), ...keys(en)])
    for (const [feature, locales] of Object.entries(byFeature)) {
      const redefined = [...keys(locales['zh-CN']!)].filter((key) => shared.has(key))
      expect({ feature, redefined }).toEqual({ feature, redefined: [] })
    }
  })
})
