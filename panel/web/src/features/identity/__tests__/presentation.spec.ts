import { describe, expect, it } from 'vitest'
import type { AccountView, TokenView } from '@/api/generated'
import { accountState, permissionKey, returnPath, tokenState } from '../presentation'

describe('returnPath', () => {
  it('keeps paths within the console', () => {
    expect(returnPath('/sites?q=shop')).toBe('/sites?q=shop')
    expect(returnPath('/account')).toBe('/account')
  })

  it('never leaves the console or returns to the login page', () => {
    for (const value of [
      undefined,
      null,
      ['/sites'],
      'https://elsewhere.example',
      '//elsewhere.example',
      '/\\elsewhere.example',
      'sites',
      '/login?next=/x',
    ]) {
      expect(returnPath(value)).toBe('/')
    }
  })
})

describe('states', () => {
  const account = { disabled: false, locked: false } as AccountView
  it('shows disabled before locked', () => {
    expect(accountState(account)).toBe('active')
    expect(accountState({ ...account, locked: true })).toBe('locked')
    expect(accountState({ ...account, locked: true, disabled: true })).toBe('disabled')
  })

  it('tells revoked and expired tokens apart', () => {
    const now = Date.parse('2026-10-03T10:00:00Z')
    const token = { expires_at: '2026-10-04T10:00:00Z', revoked_at: null } as TokenView
    expect(tokenState(token, now)).toBe('active')
    expect(tokenState({ ...token, expires_at: '2026-10-03T09:00:00Z' }, now)).toBe('expired')
    expect(tokenState({ ...token, revoked_at: '2026-10-03T09:30:00Z' }, now)).toBe('revoked')
  })

  it('keys permission descriptions without dots', () => {
    expect(permissionKey('config.read')).toBe('permissions.config_read')
  })
})
