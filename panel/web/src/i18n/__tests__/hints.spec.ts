import { createI18n } from 'vue-i18n'
import { describe, expect, it } from 'vitest'
import { messages } from '../messages'

describe('messages', () => {
  it('render variable hints verbatim in both languages', () => {
    for (const locale of ['en', 'zh-CN'] as const) {
      const i18n = createI18n({ legacy: false, locale, messages })
      const hint = i18n.global.t('sites.form.bodyHint')
      expect(hint).toContain('$host')
      expect(hint).toContain('$$')
      expect(hint).toMatch(/\$http_<(name|名称)>/)
    }
  })
})
