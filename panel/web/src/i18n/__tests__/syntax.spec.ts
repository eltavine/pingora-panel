import { createI18n } from 'vue-i18n'
import { describe, expect, it } from 'vitest'
import { messages, type Tree } from '../messages'

function* keys(tree: object, prefix = ''): Generator<string> {
  for (const [key, value] of Object.entries(tree)) {
    if (typeof value === 'string') {
      yield prefix + key
    } else {
      yield* keys(value as object, `${prefix}${key}.`)
    }
  }
}

/** The keys whose message does not compile, such as one with a bare `@`. */
function unreadable(tree: Tree): string[] {
  const { t } = createI18n({ legacy: false, locale: 'xx', messages: { xx: tree } }).global
  return Array.from(keys(tree)).filter((key) => {
    try {
      t(key, {})
      return false
    } catch {
      return true
    }
  })
}

describe('messages', () => {
  it('fail to compile with a bare `@`, which starts a linked message', () => {
    expect(unreadable({ hint: { at: 'app@sha256' } })).toEqual(['hint.at'])
  })

  it('all compile in every locale', () => {
    for (const [locale, tree] of Object.entries(messages)) {
      expect({ locale, keys: unreadable(tree) }).toEqual({ locale, keys: [] })
    }
  })
})
