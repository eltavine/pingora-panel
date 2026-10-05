import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const SITE_ID = '0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a02'

const site = {
  id: SITE_ID,
  name: 'Storefront',
  action: { type: 'respond', status: 200 },
  enabled: true,
  favorite: false,
  domains: [{ host: 'shop.example', primary: true, enabled: true }],
  routes: [],
  tags: [],
  http_policy_id: 'headers',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"s1"',
  https: false,
  kind: 'maintenance',
  status: 'running',
  unicode_hosts: { 'shop.example': 'shop.example' },
}

const headers = {
  id: 'headers',
  response: { set: [{ name: 'X-Frame-Options', value: 'DENY' }] },
  server: { mode: 'remove' },
  compression: { algorithms: ['gzip'], types: ['text/*'], min_bytes: 1024 },
  used_by: [SITE_ID],
  etag: '"h1"',
}

const spare = { id: 'spare', used_by: [], etag: '"h2"' }

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
  await page.route('**/api/v1/security-policies', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/http-policies', (route) => route.fulfill({ json: [headers, spare] }))
  await page.route('**/api/v1/http-policies/*', (route) => {
    saved.push(route.request())
    if (route.request().method() === 'DELETE') {
      return route.fulfill({ json: {} })
    }
    const body = route.request().postDataJSON() as { id: string }
    return route.fulfill({ headers: { etag: '"h3"' }, json: body })
  })
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
})

test('HTTP policies are listed, created and removed', async ({ page }) => {
  const saved: Request[] = []
  await mockConfiguration(page, saved)
  await page.goto('/http-policies')

  await expect(page.getByRole('heading', { name: 'HTTP policies' }).first()).toBeVisible()
  const row = page.getByRole('row', { name: /headers/ })
  for (const effect of ['Response fields', 'Server', 'Compression']) {
    await expect(row.getByText(effect, { exact: true })).toBeVisible()
  }
  await expect(row.getByRole('link', { name: 'Storefront' })).toBeVisible()
  await expect(row.getByRole('button', { name: 'Delete' })).toBeDisabled()

  await page.getByRole('button', { name: 'New policy' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('api')
  await sheet.getByRole('button', { name: 'Add field change' }).first().click()
  await sheet.getByLabel('Field', { exact: true }).fill('X Tenant')
  await expect(sheet.getByText('A field name such as X-Frame-Options')).toBeVisible()
  await expect(sheet.getByRole('button', { name: 'Save' })).toBeDisabled()
  await sheet.getByLabel('Field', { exact: true }).fill('X-Tenant')
  await sheet.getByLabel('Value', { exact: true }).fill('$host')
  await sheet.getByRole('combobox', { name: 'Handling' }).click()
  await page.getByRole('option', { name: 'Replace' }).click()
  await sheet.getByLabel('Server value').fill('shop')
  await sheet.getByRole('switch', { name: 'Allow cross-origin requests' }).click()
  await sheet
    .getByRole('textbox', { name: 'Allowed origins' })
    .fill('https://*.shop.example\nhttps://admin.shop.example')
  await sheet.getByRole('textbox', { name: 'Allowed methods' }).fill('PUT DELETE')
  await sheet.getByRole('switch', { name: 'Allow credentials' }).click()
  await sheet.getByLabel('Preflight cache (s)').fill('600')
  await sheet.getByRole('switch', { name: 'Compress responses' }).click()
  await sheet.getByRole('checkbox', { name: 'zstd' }).click()
  await sheet.getByLabel('Minimum size').fill('lots')
  await expect(sheet.getByText('Write a size such as 512, 1k or 1m')).toBeVisible()
  await sheet.getByLabel('Minimum size').fill('2k')
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved HTTP policy api')).toBeVisible()

  const [put] = saved
  expect(put!.method()).toBe('PUT')
  expect(put!.url()).toMatch(/\/api\/v1\/http-policies\/api$/)
  expect(put!.headers()['if-match']).toBeUndefined()
  expect(put!.postDataJSON()).toMatchObject({
    id: 'api',
    request: { remove: [], set: [{ name: 'X-Tenant', value: '$host' }], add: [] },
    server: { mode: 'replace', value: 'shop' },
    cors: {
      allowed_origins: ['https://*.shop.example', 'https://admin.shop.example'],
      allowed_methods: ['PUT', 'DELETE'],
      allow_credentials: true,
      max_age_seconds: 600,
    },
    compression: { algorithms: ['gzip', 'brotli', 'zstd'], min_bytes: 2048 },
  })

  await page.getByRole('row', { name: /spare/ }).getByRole('button', { name: 'Delete' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('HTTP policy deleted')).toBeVisible()
  expect(saved[1]!.method()).toBe('DELETE')
  expect(saved[1]!.headers()['if-match']).toBe('"h2"')
})

test('new sites name the HTTP policy their requests go through', async ({ page }) => {
  await mockConfiguration(page, [])
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
  await sheet.getByRole('combobox', { name: 'HTTP policy' }).click()
  await page.getByRole('option', { name: 'headers' }).click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Created site Wiki')).toBeVisible()
  expect(created[0]!.postDataJSON()).toMatchObject({ name: 'Wiki', http_policy_id: 'headers' })
})

test('the HTTP policy page fits a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await mockConfiguration(page, [])
  await page.goto('/http-policies')
  await expect(page.getByRole('row', { name: /headers/ })).toBeVisible()
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})
