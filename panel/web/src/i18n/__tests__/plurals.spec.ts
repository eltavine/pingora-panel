import { createI18n } from 'vue-i18n'
import { describe, expect, it } from 'vitest'
import { messages, type Locale } from '../messages'

const translate = (locale: Locale) => createI18n({ legacy: false, locale, messages }).global.t

function* entries(tree: object, prefix = ''): Generator<[string, string]> {
  for (const [key, value] of Object.entries(tree)) {
    if (typeof value === 'string') {
      yield [prefix + key, value]
    } else {
      yield* entries(value as object, `${prefix}${key}.`)
    }
  }
}

const placeholders = (form: string) => (form.match(/\{\w+\}/g) ?? []).sort()

describe('counted messages', () => {
  it('agree with their count in English', () => {
    const t = translate('en')
    expect(t('upstreams.nodeCount', { count: 1 })).toBe('1 node')
    expect(t('upstreams.nodeCount', { count: 0 })).toBe('0 nodes')
    expect(t('upstreams.nodeCount', { count: 3 })).toBe('3 nodes')
    expect(t('audit.intactDetail', { checked: 1, head: 1 }, 1)).toBe(
      '1 record checked; the chain ends at #1.',
    )
    expect(t('upstreams.invalidNodeLines', { lines: '2, 5' }, 2)).toBe('Lines 2, 5 are not valid')
  })

  it('keep a single form in Chinese', () => {
    const t = translate('zh-CN')
    expect(t('upstreams.nodeCount', { count: 1 })).toBe('1 个节点')
    expect(t('upstreams.nodeCount', { count: 3 })).toBe('3 个节点')
    const plural = Array.from(entries(messages['zh-CN']))
      .filter(([, message]) => message.includes(' | '))
      .map(([key]) => key)
    expect(plural).toEqual([])
  })

  it('give every English plural one singular and one plural form', () => {
    const malformed = Array.from(entries(messages.en))
      .map(([key, message]) => [key, message.split(' | ')] as const)
      .filter(([, forms]) => forms.length > 1)
      .filter(
        ([, forms]) =>
          forms.length !== 2 || placeholders(forms[0]!).join() !== placeholders(forms[1]!).join(),
      )
      .map(([key]) => key)
    expect(malformed).toEqual([])
  })
})
