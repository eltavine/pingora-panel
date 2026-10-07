import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

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

/** Docker running shop-web-1, published on 8081, which the Shop site points at. */
async function mockContainers(page: Page) {
  await page.route(/\/api\/v1\/container-engines$/, (route) =>
    route.fulfill({
      json: {
        engines: [
          {
            id: 'docker',
            socket: '/run/docker.sock',
            enabled: true,
            reachable: true,
            detail: null,
            version: null,
            info: null,
          },
        ],
      },
    }),
  )
  await page.route(/\/api\/v1\/container-engines\/docker\/containers\?/, (route) =>
    route.fulfill({
      json: {
        observed_at: '2026-10-04T10:00:00Z',
        containers: [
          {
            id: '4f1c2a9be03d71aa',
            names: ['shop-web-1'],
            image: 'nginx:1.27',
            image_id: 'sha256:4f1c',
            created: null,
            state: 'running',
            status: 'Up 26 hours',
            ports: [],
            labels: {},
            compose_project: null,
            addresses: [],
            endpoints: [
              {
                host: '127.0.0.1',
                port: 8081,
                container_port: 80,
                route: 'published',
                network: null,
              },
            ],
          },
        ],
      },
    }),
  )
  await page.route(/\/api\/v1\/container-engines\/docker\/site-links$/, (route) =>
    route.fulfill({
      json: {
        observed_at: '2026-10-04T10:00:00Z',
        links: [
          {
            container_id: '4f1c2a9be03d71aa',
            container: 'shop-web-1',
            site_id: SITE_ID,
            site: 'Shop',
            upstream_id: UPSTREAM_ID,
            upstream: 'shop-backend',
            node: '127.0.0.1:8081',
            route: 'published',
          },
        ],
        unserved: [],
      },
    }),
  )
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
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

  // Opening a select renders reka-ui's inline viewport style under the production policy.
  await page.getByRole('combobox', { name: 'Status' }).click()
  await page.getByRole('option', { name: 'Running' }).click()
  await expect(page.getByRole('combobox', { name: 'Status' })).toContainText('Running')

  await page.getByRole('button', { name: 'New site' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Site name').fill('Blog')
  await sheet.getByRole('radio', { name: 'Static site' }).click()
  await sheet.getByLabel('Site directory').fill('blog')
  await sheet.getByLabel('Domains').fill('blog.example\nwww.blog.example')
  await sheet.getByRole('combobox', { name: 'Format' }).click()
  await page.getByRole('option', { name: 'Combined (NGINX)' }).click()
  const fields = sheet.getByLabel('Extra fields')
  await fields.fill('tenant.id $http_x_tenant')
  await expect(sheet.getByRole('alert')).toHaveText('Line 1 is not name = template')
  await expect(sheet.getByRole('button', { name: 'Create' })).toBeDisabled()
  await fields.fill('tenant.id = $http_x_tenant')
  await sheet.getByRole('button', { name: 'Add a page' }).click()
  await page.getByRole('menuitem', { name: '503 Unavailable' }).click()
  await sheet.getByLabel('Page body').fill('<h1>Back soon</h1>')
  await sheet.getByRole('switch', { name: 'In maintenance' }).click()
  const allowed = sheet.getByLabel('Clients that still reach the site')
  await allowed.fill('10.0.0.0/8\noffice')
  await expect(sheet.getByRole('alert')).toHaveText('Line 2 is not a network or address.')
  await expect(sheet.getByRole('button', { name: 'Create' })).toBeDisabled()
  await allowed.fill('10.0.0.0/8')
  await sheet.getByRole('combobox', { name: 'robots.txt' }).click()
  await page.getByRole('option', { name: 'Disallow every crawler' }).click()
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
    access_log: { enabled: null, format: 'combined', fields: { 'tenant.id': '$http_x_tenant' } },
    error_pages: {
      pages: [{ statuses: [503], response: { kind: 'body', body: '<h1>Back soon</h1>' } }],
      intercept: false,
    },
    maintenance: { enabled: true, allow: ['10.0.0.0/8'], status: 503 },
    robots: { kind: 'disallow_all' },
    favicon: null,
  })
})

test('a pending draft is validated before it is applied', async ({ page }) => {
  await mockConfiguration(page, true)
  await page.route('**/api/v1/config/validation*', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  await page.route('**/api/v1/config/plan', (route) =>
    route.fulfill({
      json: {
        resources: [{ resource: `sites/${SITE_ID}`, change: 'added', diff: '+server shop {\n' }],
        files: [],
        digest: 'f'.repeat(64),
        draft_version: 4,
        active_revision: 2,
      },
    }),
  )
  const applied: Request[] = []
  await page.route('**/api/v1/config/apply', (route) => {
    applied.push(route.request())
    return route.fulfill({
      json: {
        draft: { version: 4, pending: false, applied_version: 4 },
        revision: 3,
        revision_id: 9,
        content_hash: 'c'.repeat(64),
      },
    })
  })

  await page.goto('/sites')
  await page.getByRole('button', { name: 'Apply' }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog).toContainText('The draft is valid')
  await expect(dialog).toContainText('These changes replace revision #2 on the gateway.')
  await expect(dialog.getByRole('list', { name: 'What changes' })).toContainText('1 added')
  await dialog.getByRole('button', { name: 'Apply' }).click()
  await expect(page.getByText('Applied as revision #3')).toBeVisible()
  expect(applied[0]!.postDataJSON()).toEqual({ expected_version: 4, expected_plan: 'f'.repeat(64) })
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

const API_ID = '8f2a1d4b-6c3e-4d9f-a071-2b3c4d5e6f70'
const API_NODE_ID = '4d3c2b1a-9f8e-4d7c-b6a5-1f2e3d4c5b6a'

/** Shop sends /api to its own pool and everything else to shop-backend. */
async function mockTopology(page: Page) {
  await mockConfiguration(page)
  await page.route(/\/api\/v1\/sites\?/, (route) =>
    route.fulfill({
      json: {
        items: [
          {
            ...site,
            routes: [
              {
                id: 'r1',
                enabled: true,
                priority: 1,
                name: 'api',
                match: { kind: 'prefix', path: '/api' },
                action: { type: 'proxy', upstream_id: API_ID },
              },
            ],
          },
        ],
        total: 1,
        next_cursor: null,
      },
    }),
  )
  await page.route('**/api/v1/upstreams', (route) =>
    route.fulfill({
      json: [
        upstream,
        {
          ...upstream,
          id: API_ID,
          name: 'api-backend',
          nodes: [{ id: API_NODE_ID, host: '10.0.0.21', port: 9000, enabled: true }],
          health_check: null,
        },
      ],
    }),
  )
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
                drained: false,
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
}

test('the topology shows how sites reach pools and their nodes', async ({ page }) => {
  await mockTopology(page)
  await page.goto('/upstreams?view=topology')
  await expect(page.getByRole('tab', { name: 'Topology' })).toHaveAttribute('aria-selected', 'true')
  for (const column of ['Sites', 'Routes', 'Pools', 'Nodes']) {
    await expect(page.getByRole('heading', { name: column, exact: true })).toBeVisible()
  }
  const sites = page.getByRole('region', { name: 'Sites' })
  await expect(sites.getByRole('link', { name: 'Shop' })).toBeVisible()
  await expect(sites).toContainText('Requests no route takes go to shop-backend')
  const routes = page.getByRole('region', { name: 'Routes' })
  await expect(routes).toContainText('/api')
  await expect(routes).toContainText('api · Shop')
  await expect(routes).toContainText('Sends to api-backend')
  const nodes = page.getByRole('region', { name: 'Nodes' })
  await expect(nodes.getByRole('listitem').filter({ hasText: '10.0.0.11:8080' })).toContainText(
    'Healthy',
  )
  await expect(nodes.getByRole('listitem').filter({ hasText: '10.0.0.11:8080' })).toContainText(
    '1.83 ms · 2 in flight',
  )
  await expect(nodes.getByRole('listitem').filter({ hasText: '10.0.0.21:9000' })).toContainText(
    'Not checked',
  )
  await expect(page.getByRole('region', { name: 'Pools' })).toContainText('1/1 healthy')

  await expectNoHorizontalOverflow(page)

  await page.getByRole('tab', { name: 'Pools' }).click()
  await expect(page).not.toHaveURL(/view=/)
  await expect(page.getByRole('link', { name: 'api-backend' })).toBeVisible()
})

test('wide screens join the topology with connectors and narrow ones read it as lists', async ({
  page,
}) => {
  await mockTopology(page)
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.goto('/upstreams?view=topology')
  const connectors = page.getByTestId('topology-connectors').locator('path')
  await expect(connectors).toHaveCount(5)
  await page.getByRole('link', { name: 'api-backend' }).hover()
  await expect(connectors.and(page.locator('.stroke-foreground'))).toHaveCount(2)

  await page.setViewportSize({ width: 390, height: 844 })
  await expect(connectors).toHaveCount(0)
  await expect(page.getByRole('region', { name: 'Routes' })).toContainText('Sends to api-backend')
  await expectNoHorizontalOverflow(page)
})

test('the topology says when no site reaches an upstream or sites cannot be read', async ({
  page,
}) => {
  await mockTopology(page)
  let reads = 0
  await page.route(/\/api\/v1\/sites\?/, (route) => {
    reads += 1
    if (reads <= 2) {
      return route.fulfill({
        status: 503,
        contentType: 'application/problem+json',
        json: {
          type: 'about:blank',
          title: 'Service Unavailable',
          status: 503,
          code: 'SERVICE_UNAVAILABLE',
          detail: 'the configuration service is starting',
        },
      })
    }
    return route.fulfill({ json: { items: [], total: 0, next_cursor: null } })
  })
  await page.goto('/upstreams?view=topology')
  const failure = page
    .getByRole('alert')
    .filter({ hasText: 'the configuration service is starting' })
  await expect(failure).toBeVisible()
  await failure.getByRole('button', { name: 'Retry' }).click()
  await expect(page.getByText('No live site sends traffic to an upstream yet')).toBeVisible()
  await expect(page.getByText('No route sends to an upstream of its own')).toBeVisible()
  await expect(page.getByRole('region', { name: 'Pools' })).toContainText('Not used')
  await expectNoHorizontalOverflow(page)
})

test('listeners start from explanatory empty states', async ({ page }) => {
  await mockConfiguration(page)
  await page.goto('/listeners')

  await expect(page.getByText('No listeners yet')).toBeVisible()
  await expect(page.getByText('No TLS profiles yet')).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('a node is pointed at a running container', async ({ page }) => {
  await mockConfiguration(page)
  await mockContainers(page)
  await page.route(`**/api/v1/upstreams/${UPSTREAM_ID}`, (route) =>
    route.fulfill({ json: upstream }),
  )
  await page.route('**/api/v1/upstreams/health', (route) =>
    route.fulfill({ json: { upstreams: [] } }),
  )
  await page.goto(`/upstreams/${UPSTREAM_ID}`)
  await page.getByRole('button', { name: 'Add node' }).click()
  const sheet = page.getByRole('dialog', { name: 'Add node' })
  await sheet.getByRole('combobox', { name: 'From a container' }).click()
  await page.getByRole('option', { name: /shop-web-1/ }).click()
  await expect(sheet.getByLabel('Host')).toHaveValue('127.0.0.1')
  await expect(sheet.getByLabel('Port')).toHaveValue('8081')
})

test('a site lists the containers behind it', async ({ page }) => {
  await mockConfiguration(page)
  await mockContainers(page)
  await page.route(`**/api/v1/sites/${SITE_ID}`, (route) => route.fulfill({ json: site }))
  await page.goto(`/sites/${SITE_ID}`)
  const card = page.locator('[data-slot="card"]').filter({ hasText: 'Containers' })
  await expect(card.getByRole('link', { name: 'shop-web-1' })).toHaveAttribute(
    'href',
    '/containers?engine=docker&search=shop-web-1',
  )
  await expect(card).toContainText('127.0.0.1:8081')
  await expect(card).toContainText('Published port')
  await expect(card).toContainText('through shop-backend')
})
