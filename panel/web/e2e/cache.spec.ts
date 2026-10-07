import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const SITE_ID = '0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a03'

const site = {
  id: SITE_ID,
  name: 'Storefront',
  action: { type: 'respond', status: 200 },
  enabled: true,
  favorite: false,
  domains: [{ host: 'shop.example', primary: true, enabled: true }],
  routes: [],
  tags: [],
  cache_policy_id: 'pages',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"s1"',
  https: false,
  kind: 'maintenance',
  status: 'running',
  unicode_hosts: { 'shop.example': 'shop.example' },
}

const pages = {
  id: 'pages',
  ttl_seconds: 600,
  status_ttls: { '404': 60 },
  bypass: [{ kind: 'cookie', name: 'session', test: { op: 'present' } }],
  used_by: [SITE_ID],
  etag: '"c1"',
}

const spare = { id: 'spare', enabled: false, used_by: [], etag: '"c2"' }

const stats = {
  observed_at: '2026-10-07T10:00:00Z',
  since: '2026-10-07T09:00:00Z',
  bytes: 512 * 1024,
  entries: 12,
  max_bytes: 256 * 1024 ** 2,
  sites: [
    {
      site_id: SITE_ID,
      hits: 6,
      stale: 0,
      updating: 0,
      misses: 2,
      expired: 0,
      revalidated: 0,
      bypasses: 5,
      uncacheable: 0,
      hit_ratio: 0.75,
    },
  ],
}

interface Seen {
  policies: Request[]
  purges: Request[]
  store: Request[]
}

async function mockCache(page: Page): Promise<Seen> {
  const seen: Seen = { policies: [], purges: [], store: [] }
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route(/\/api\/v1\/sites\?/, (route) =>
    route.fulfill({ json: { items: [site], total: 1, next_cursor: null } }),
  )
  await page.route('**/api/v1/gateway/cache', (route) => route.fulfill({ json: stats }))
  await page.route('**/api/v1/gateway/cache/purge', (route) => {
    seen.purges.push(route.request())
    return route.fulfill({ json: { keys: 2 } })
  })
  await page.route('**/api/v1/cache-settings', (route) => {
    if (route.request().method() === 'PUT') {
      seen.store.push(route.request())
      return route.fulfill({ json: route.request().postDataJSON() })
    }
    return route.fulfill({ headers: { etag: '"store"' }, json: {} })
  })
  await page.route('**/api/v1/cache-policies', (route) => route.fulfill({ json: [pages, spare] }))
  await page.route('**/api/v1/cache-policies/*', (route) => {
    seen.policies.push(route.request())
    if (route.request().method() === 'DELETE') {
      return route.fulfill({ json: {} })
    }
    return route.fulfill({ headers: { etag: '"c3"' }, json: route.request().postDataJSON() })
  })
  return seen
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
})

test('the cache shows what it holds and is purged by URL, site or whole', async ({ page }) => {
  const seen = await mockCache(page)
  await page.goto('/cache')

  await expect(page.getByRole('heading', { name: 'Cache', exact: true }).first()).toBeVisible()
  await expect(page.getByText('512 KiB of 256 MiB')).toBeVisible()
  const siteRow = page.getByRole('row', { name: /Storefront/ })
  await expect(siteRow.getByText('75%')).toBeVisible()

  await page.getByLabel('URLs to purge').fill('https://shop.example/\nhttps://shop.example/a')
  await page.getByRole('button', { name: 'Purge URLs' }).click()
  await expect(page.getByText('Purged 2 keys')).toBeVisible()
  expect(seen.purges[0]!.postDataJSON()).toEqual({
    urls: ['https://shop.example/', 'https://shop.example/a'],
  })

  await siteRow.getByRole('button', { name: 'Purge site' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Purge site' }).click()
  await expect.poll(() => seen.purges.length).toBe(2)
  expect(seen.purges[1]!.postDataJSON()).toEqual({ site_ids: [SITE_ID] })

  await page.getByRole('button', { name: 'Purge everything' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Purge everything' }).click()
  await expect.poll(() => seen.purges.length).toBe(3)
  expect(seen.purges[2]!.postDataJSON()).toEqual({ all: true })

  const size = page.getByRole('textbox', { name: 'Size' })
  await size.fill('512k')
  await expect(page.getByText('Write a size between 1m and 64g')).toBeVisible()
  await size.fill('1g')
  await page.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved the cache size')).toBeVisible()
  expect(seen.store[0]!.postDataJSON()).toEqual({ max_bytes: 1024 ** 3 })
})

test('cache policies are listed, created and removed', async ({ page }) => {
  const seen = await mockCache(page)
  await page.goto('/cache')

  const row = page.getByRole('row', { name: /pages/ })
  await expect(row.getByText('10m')).toBeVisible()
  await expect(row.getByText('404=1m')).toBeVisible()
  await expect(row.getByText('1 bypass')).toBeVisible()
  await expect(row.getByRole('link', { name: 'Storefront' })).toBeVisible()
  await expect(row.getByRole('button', { name: 'Delete' })).toBeDisabled()
  await expect(page.getByRole('row', { name: /spare/ }).getByText('Disabled')).toBeVisible()

  await page.getByRole('button', { name: 'New policy' }).click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Identifier').fill('off')
  await expect(sheet.getByRole('button', { name: 'Save' })).toBeDisabled()
  await sheet.getByLabel('Identifier').fill('assets')
  await sheet.getByLabel('Fresh for', { exact: true }).first().fill('1h')
  await sheet.getByRole('button', { name: 'Add statuses' }).click()
  await sheet.getByLabel('Statuses').fill('404 410')
  await sheet.getByLabel('Fresh for', { exact: true }).nth(1).fill('1m')
  await sheet.getByLabel('Vary by').fill('Accept-Language')
  await sheet.getByRole('switch', { name: "Follow the origin's Cache-Control and Expires" }).click()
  await sheet.getByRole('button', { name: 'Add a condition' }).click()
  await page.getByRole('menuitem', { name: 'Cookie' }).click()
  await sheet.getByLabel('Name', { exact: true }).fill('session')
  await sheet.getByLabel('Largest response').fill('128m')
  await expect(sheet.getByText('Write a size such as 8m, at most 64m')).toBeVisible()
  await sheet.getByLabel('Largest response').fill('16m')
  await sheet.getByRole('button', { name: 'Save' }).click()
  await expect(page.getByText('Saved cache policy assets')).toBeVisible()

  const [put] = seen.policies
  expect(put!.method()).toBe('PUT')
  expect(put!.url()).toMatch(/\/api\/v1\/cache-policies\/assets$/)
  expect(put!.headers()['if-match']).toBeUndefined()
  expect(put!.postDataJSON()).toMatchObject({
    id: 'assets',
    ttl_seconds: 3_600,
    status_ttls: { '404': 60, '410': 60 },
    vary_headers: ['accept-language'],
    honor_origin: false,
    max_object_bytes: 16 * 1024 ** 2,
    bypass: [{ kind: 'cookie', name: 'session' }],
  })

  await page.getByRole('row', { name: /spare/ }).getByRole('button', { name: 'Delete' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete' }).click()
  await expect(page.getByText('Cache policy deleted')).toBeVisible()
  expect(seen.policies[1]!.method()).toBe('DELETE')
  expect(seen.policies[1]!.headers()['if-match']).toBe('"c2"')
})

test('the cache page fits a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await mockCache(page)
  await page.goto('/cache')
  await expect(page.getByRole('row', { name: /pages/ })).toBeVisible()
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})
