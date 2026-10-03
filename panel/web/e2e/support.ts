import type { Page } from '@playwright/test'

/** Every permission of the catalog. */
export const ALL_PERMISSIONS = [
  'gateway.read',
  'gateway.operate',
  'gateway.publish',
  'config.read',
  'config.write',
  'config.apply',
  'audit.read',
  'platform.read',
  'identity.read',
  'identity.manage',
]

export const CSRF_TOKEN = 'csrf-token-of-the-session'

/** The session the console sees for an account holding `permissions`. */
export function currentSession(permissions: string[] = ALL_PERMISSIONS, username = 'root') {
  return {
    account: {
      id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a50',
      username,
      display_name: null,
      disabled: false,
      locked: false,
      roles: ['administrator'],
      created_at: '2026-10-01T08:00:00Z',
      updated_at: '2026-10-01T08:00:00Z',
      last_login_at: '2026-10-03T08:00:00Z',
      password_changed_at: '2026-10-01T08:00:00Z',
    },
    permissions,
    credential: 'cookie',
    session: {
      id: '0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a51',
      transport: 'cookie',
      created_at: '2026-10-03T08:00:00Z',
      last_seen_at: '2026-10-03T08:30:00Z',
      idle_until: '2026-10-03T09:30:00Z',
      expires_at: '2026-10-04T08:00:00Z',
      client_address: '127.0.0.1',
      user_agent: 'Playwright',
      current: true,
    },
    csrf_token: CSRF_TOKEN,
  }
}

/** Logs the browser in as an account holding `permissions`. */
export async function signIn(page: Page, permissions: string[] = ALL_PERMISSIONS) {
  await page.route('**/api/v1/session', (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({ json: currentSession(permissions) })
      : route.fallback(),
  )
}
