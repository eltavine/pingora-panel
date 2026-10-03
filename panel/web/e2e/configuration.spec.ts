import { expect, test, type Page, type Request } from '@playwright/test'

const SITE_ID = '0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a01'
const UPSTREAM_ID = '7e1f0c3a-5b2d-4c8e-9f60-1a2b3c4d5e6f'
const NODE_ID = '3c2b1a09-8f7e-4d6c-b5a4-0f1e2d3c4b5a'

const upstream = {
  id: UPSTREAM_ID,
  name: 'shop-backend',
  balancing: 'round_robin',
  nodes: [{ id: NODE_ID, host: '10.0.0.11', port: 8080, weight: 3, enabled: true }],
  health_check: {
    protocol: 'http',
    path: '/healthz',
    method: 'GET',
    interval_ms: 5000,
    timeout_ms: 1000,
    healthy_threshold: 2,
    unhealthy_threshold: 3,
  },
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"u1"',
  used_by: [SITE_ID],
}

const site = {
  id: SITE_ID,
  name: 'Shop',
  action: { type: 'proxy', upstream_id: UPSTREAM_ID },
  enabled: true,
  favorite: false,
  domains: [{ host: 'shop.example', primary: true, enabled: true }],
  routes: [],
  tags: ['prod'],
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"s1"',
  https: false,
  kind: 'reverse_proxy',
  status: 'running',
  unicode_hosts: { 'shop.example': 'shop.example' },
}

async function useEnglish(page: Page) {
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
}

async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
}

async function mockConfiguration(page: Page, pending = false) {
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending, applied_version: pending ? 3 : 4 } }),
  )
  await page.route('**/api/v1/sites/summary', (route) =>
    route.fulfill({
      json: {
        total: 1,
        running: 1,
        stopped: 0,
        abnormal: 0,
        https: 0,
        reverse_proxy: 1,
        static: 0,
        redirect: 0,
        maintenance: 0,
        deleted: 0,
      },
    }),
  )
  await page.route(/\/api\/v1\/sites\?/, (route) =>
    route.fulfill({ json: { items: [site], total: 1, next_cursor: null } }),
  )
  await page.route('**/api/v1/upstreams', (route) =>
    route.request().method() === 'GET' ? route.fulfill({ json: [upstream] }) : route.fallback(),
  )
  await page.route('**/api/v1/listeners', (route) => route.fulfill({ json: [] }))
  await page.route('**/api/v1/tls-profiles', (route) => route.fulfill({ json: [] }))
}

test.beforeEach(async ({ page }) => {
  await useEnglish(page)
  await page.addInitScript(() => {
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
})

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('sites are summarized, listed and created', async ({ page }) => {
  await mockConfiguration(page)
  const created: Request[] = []
  await page.route('**/api/v1/sites', (route) => {
    if (route.request().method() !== 'POST') {
      return route.fallback()
    }
    created.push(route.request())
    return route.fulfill({
      status: 201,
      json: { ...site, id: 'new', name: 'Blog', etag: '"s2"', domains: [] },
    })
  })

  await page.goto('/sites')
  await expect(page.getByRole('heading', { name: 'Sites' })).toBeVisible()
  await expect(page.getByRole('button', { name: /All sites/ })).toContainText('1')
  await expect(page.getByRole('link', { name: 'Shop' })).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Running' })).toBeVisible()
  await expectNoHorizontalOverflow(page)

  await page.getByRole('button', { name: 'New site' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Site name').fill('Blog')
  await sheet.getByRole('radio', { name: 'Static site' }).click()
  await sheet.getByLabel('Site directory').fill('blog')
  await sheet.getByLabel('Domains').fill('blog.example\nwww.blog.example')
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect(page.getByText('Created site Blog')).toBeVisible()

  const [request] = created
  expect(request!.headers()['idempotency-key']).toMatch(/^[\x21-\x7e]+$/)
  expect(request!.postDataJSON()).toMatchObject({
    name: 'Blog',
    action: { type: 'static', root: 'blog', index_files: ['index.html'], spa_fallback: false },
    domains: [
      { host: 'blog.example', primary: true, enabled: true },
      { host: 'www.blog.example', primary: false, enabled: true },
    ],
  })
})

test('a pending draft is validated before it is applied', async ({ page }) => {
  await mockConfiguration(page, true)
  await page.route('**/api/v1/config/validation*', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  const applied: Request[] = []
  await page.route('**/api/v1/config/apply', (route) => {
    applied.push(route.request())
    return route.fulfill({
      json: {
        draft: { version: 4, pending: false, applied_version: 4 },
        revision_id: 9,
        content_hash: 'c'.repeat(64),
      },
    })
  })

  await page.goto('/sites')
  await page.getByRole('button', { name: 'Apply' }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog).toContainText('The draft is valid')
  await dialog.getByRole('button', { name: 'Apply' }).click()
  await expect(page.getByText('Applied v4')).toBeVisible()
  expect(applied[0]!.postDataJSON()).toEqual({ expected_version: 4 })
})

test('upstream nodes show live health and can be drained', async ({ page }) => {
  await mockConfiguration(page)
  await page.route(`**/api/v1/upstreams/${UPSTREAM_ID}`, (route) =>
    route.fulfill({ json: upstream }),
  )
  let drained = false
  await page.route('**/api/v1/upstreams/health', (route) =>
    route.fulfill({
      json: {
        upstreams: [
          {
            upstream_id: UPSTREAM_ID,
            checked: true,
            nodes: [
              {
                node_id: NODE_ID,
                address: '10.0.0.11:8080',
                weight: 3,
                enabled: true,
                backup: false,
                healthy: true,
                drained,
                in_flight: 2,
                requests: 1200,
                failures: 4,
                latency_us: 1830,
              },
            ],
          },
        ],
      },
    }),
  )
  await page.route(`**/api/v1/upstreams/${UPSTREAM_ID}/nodes/${NODE_ID}/drain`, (route) => {
    drained = true
    return route.fulfill({ json: {} })
  })

  await page.goto(`/upstreams/${UPSTREAM_ID}`)
  await expect(page.getByRole('heading', { name: 'shop-backend' })).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Healthy' })).toBeVisible()
  await expect(page.getByText('1.83 ms')).toBeVisible()

  await page.getByRole('button', { name: 'Drain' }).click()
  await expect(page.getByText('The node is drained; new requests avoid it')).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Drained' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('listeners start from explanatory empty states', async ({ page }) => {
  await mockConfiguration(page)
  await page.goto('/listeners')

  await expect(page.getByText('No listeners yet')).toBeVisible()
  await expect(page.getByText('No TLS profiles yet')).toBeVisible()
  await expectNoHorizontalOverflow(page)
})
