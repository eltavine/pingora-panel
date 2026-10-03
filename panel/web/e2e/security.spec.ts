import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const SITE_ID = '0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a01'

const site = {
  id: SITE_ID,
  name: 'Intranet',
  action: { type: 'respond', status: 200 },
  enabled: true,
  favorite: false,
  domains: [{ host: 'intranet.example', primary: true, enabled: true }],
  routes: [],
  tags: [],
  security_policy_id: 'office',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"s1"',
  https: false,
  kind: 'maintenance',
  status: 'running',
  unicode_hosts: { 'intranet.example': 'intranet.example' },
}

const office = {
  id: 'office',
  allowed_cidrs: ['10.0.0.0/8'],
  basic_auth: { realm: 'Staff', users_secret_id: 'staff.htpasswd' },
  rate_limits: [{ key: { kind: 'client_address' }, requests: 10, per_seconds: 1, burst: 20 }],
  used_by: [SITE_ID],
  etag: '"p1"',
}

const open = { id: 'open', used_by: [], etag: '"p2"' }

async function mockConfiguration(page: Page, saved: Request[]) {
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route(/\/api\/v1\/sites\?/, (route) =>
    route.fulfill({ json: { items: [site], total: 1, next_cursor: null } }),
  )
  await page.route('**/api/v1/sites/summary', (route) =>
    route.fulfill({
      json: {
        total: 1,
        running: 1,
        stopped: 0,
        abnormal: 0,
        https: 0,
        reverse_proxy: 0,
        static: 0,
        redirect: 0,
        maintenance: 1,
        deleted: 0,
      },
    }),
  )
  await page.route('**/api/v1/upstreams', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/listeners', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/tls-profiles', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/security-policies', (route) =>
    route.fulfill({ json: [office, open] }),
  )
  await page.route('**/api/v1/security-policies/*', (route) => {
    saved.push(route.request())
    if (route.request().method() === 'DELETE') {
      return route.fulfill({ json: {} })
    }
    const body = route.request().postDataJSON() as { id: string }
    return route.fulfill({ headers: { etag: '"p3"' }, json: { ...body, id: body.id } })
  })
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
})

test('security policies are listed, created and removed', async ({ page }) => {
  const saved: Request[] = []
  await mockConfiguration(page, saved)
  await page.goto('/security-policies')

  await expect(page.getByRole('heading', { name: 'Security policies' }).first()).toBeVisible()
  const row = page.getByRole('row', { name: /office/ })
  for (const restriction of ['Networks', 'Password', 'Rates']) {
    await expect(row.getByText(restriction)).toBeVisible()
  }
  await expect(row.getByRole('link', { name: 'Intranet' })).toBeVisible()
  await expect(row.getByRole('button', { name: 'Delete' })).toBeDisabled()

  await page.getByRole('button', { name: 'New policy' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('api-limits')
  await sheet.getByLabel('Allowed networks').fill('10.0.0.0/8\n192.0.2.7')
  await sheet.getByRole('checkbox', { name: 'GET' }).click()
  await sheet.getByRole('switch', { name: 'Require a password' }).click()
  await sheet.getByLabel('htpasswd file').fill('staff.htpasswd')
  await sheet.getByLabel('Body limit').fill('ten')
  await expect(sheet.getByText('Write a size such as 16k or 10m')).toBeVisible()
  await expect(sheet.getByRole('button', { name: 'Save' })).toBeDisabled()
  await sheet.getByLabel('Body limit').fill('10m')
  await sheet.getByRole('button', { name: 'Add rate limit' }).click()
  await sheet.getByLabel('Requests', { exact: true }).fill('5')
  await sheet.getByRole('combobox', { name: 'Per' }).click()
  await page.getByRole('option', { name: 'minute' }).click()
  await sheet.getByRole('combobox', { name: 'Counted by' }).click()
  await page.getByRole('option', { name: 'Request header' }).click()
  await sheet.getByLabel('Header name').fill('X-Api-Key')
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved security policy api-limits')).toBeVisible()

  const [put] = saved
  expect(put!.method()).toBe('PUT')
  expect(put!.url()).toMatch(/\/api\/v1\/security-policies\/api-limits$/)
  expect(put!.headers()['if-match']).toBeUndefined()
  expect(put!.postDataJSON()).toMatchObject({
    id: 'api-limits',
    allowed_cidrs: ['10.0.0.0/8', '192.0.2.7'],
    allowed_methods: ['GET'],
    basic_auth: { realm: 'Restricted', users_secret_id: 'staff.htpasswd' },
    max_body_bytes: 10_485_760,
    rate_limits: [
      { key: { kind: 'header', name: 'X-Api-Key' }, requests: 5, per_seconds: 60, burst: 0 },
    ],
    referer: null,
  })

  await page.getByRole('row', { name: /open/ }).getByRole('button', { name: 'Delete' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Security policy deleted')).toBeVisible()
  expect(saved[1]!.method()).toBe('DELETE')
  expect(saved[1]!.headers()['if-match']).toBe('"p2"')
})

test('new sites name the policy their requests pass', async ({ page }) => {
  const saved: Request[] = []
  await mockConfiguration(page, saved)
  const created: Request[] = []
  await page.route('**/api/v1/sites', (route) => {
    if (route.request().method() !== 'POST') {
      return route.fallback()
    }
    created.push(route.request())
    return route.fulfill({ status: 201, json: { ...site, id: 'new', name: 'Wiki' } })
  })
  await page.goto('/sites')
  await page.getByRole('button', { name: 'New site' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Site name').fill('Wiki')
  await sheet.getByRole('radio', { name: 'Static site' }).click()
  await sheet.getByLabel('Site directory').fill('wiki')
  await sheet.getByRole('combobox', { name: 'Security policy' }).click()
  await page.getByRole('option', { name: 'office' }).click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Created site Wiki')).toBeVisible()
  expect(created[0]!.postDataJSON()).toMatchObject({ name: 'Wiki', security_policy_id: 'office' })
})

test('the policy page fits a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await mockConfiguration(page, [])
  await page.goto('/security-policies')
  await expect(page.getByRole('row', { name: /office/ })).toBeVisible()
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})
