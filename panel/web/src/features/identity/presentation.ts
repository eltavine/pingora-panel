import type { AccountView, TokenView } from '@/api/generated'

export type AccountState = 'active' | 'locked' | 'disabled'

export function accountState(account: AccountView): AccountState {
  if (account.disabled) {
    return 'disabled'
  }
  return account.locked ? 'locked' : 'active'
}

export type TokenState = 'active' | 'revoked' | 'expired'

export function tokenState(token: TokenView, now: number = Date.now()): TokenState {
  if (token.revoked_at) {
    return 'revoked'
  }
  return Date.parse(token.expires_at) <= now ? 'expired' : 'active'
}

/** The i18n key describing a permission; message keys cannot hold dots. */
export function permissionKey(name: string): string {
  return `permissions.${name.replace(/\./g, '_')}`
}

/**
 * Where to go after logging in: a path within the console. Anything else,
 * such as `//elsewhere.example`, would send the visitor to another site.
 */
export function returnPath(value: unknown): string {
  if (typeof value !== 'string' || !value.startsWith('/')) {
    return '/'
  }
  if (value.startsWith('//') || value.startsWith('/\\') || value.startsWith('/login')) {
    return '/'
  }
  return value
}

/** Token lifetimes offered, in days. */
export const TOKEN_LIFETIMES = [7, 30, 90, 365] as const
