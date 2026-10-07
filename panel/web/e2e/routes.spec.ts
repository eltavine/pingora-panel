import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const SITE_ID = '0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a01'
const UPSTREAM_ID = '7e1f0c3a-5b2d-4c8e-9f60-1a2b3c4d5e6f'
const ROUTE_ID = '5d1c7a52-2b8e-4d6b-9a33-0d3c58f1e2b5'

const route = {
  id: ROUTE_ID,
  name: 'canary',
  enabled: true,
  priority: 10,
  match: {
    kind: 'prefix',
    path: '/api',
    host: null,
    conditions: [
      { kind: 'method', methods: ['GET'] },
      { kind: 'header', name: 'x-canary', test: { op: 'equals', value: '1' } },
    ],
  },
  action: { type: 'proxy', upstream_id: UPSTREAM_ID },
  etag: '"r1"',
}

const site = {
  id: SITE_ID,
  name: 'Shop',
  action: { type: 'proxy', upstream_id: UPSTREAM_ID },
  enabled: true,
  favorite: false,
  domains: [{ host: 'shop.example', primary: true, enabled: true }],
  routes: [route],
  tags: [],
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"s1"',
  https: false,
  kind: 'reverse_proxy',
  status: 'running',
  unicode_hosts: { 'shop.example': 'shop.example' },
}

async function setUp(page: Page) {
  const seen: { created: Request[]; tested: Request[] } = { created: [], tested: [] }
  await signIn(page)
  await page.addInitScript(() => {
    window.localStorage.setItem('pingora-panel.locale', 'en')
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
  await page.route('**/api/v1/config/draft', (handled) =>
    handled.fulfill({ json: { version: 9, pending: true, applied_version: 8 } }),
  )
  await page.route(`**/api/v1/sites/${SITE_ID}`, (handled) => handled.fulfill({ json: site }))
  await page.route(`**/api/v1/sites/${SITE_ID}/routes`, (handled) => {
    if (handled.request().method() === 'POST') {
      seen.created.push(handled.request())
      return handled.fulfill({ status: 201, json: { ...route, id: 'new-route' } })
    }
    return handled.fulfill({ json: [route] })
  })
  await page.route('**/api/v1/upstreams', (handled) =>
    handled.fulfill({
      json: [
        {
          id: UPSTREAM_ID,
          name: 'shop-backend',
          balancing: 'round_robin',
          nodes: [],
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-02T00:00:00Z',
          etag: '"u1"',
          used_by: [SITE_ID],
        },
      ],
    }),
  )
  await page.route('**/api/v1/security-policies', (handled) => handled.fulfill({ json: [] }))
  await page.route('**/api/v1/config/route-test', (handled) => {
    seen.tested.push(handled.request())
    return handled.fulfill({
      json: {
        outcome: 'routed',
        draft_version: 9,
        host: 'shop.example',
        path: '/api/items',
        site_id: SITE_ID,
        default_site: false,
        route_id: `${SITE_ID}-site`,
        routes: [
          {
            route_id: ROUTE_ID,
            name: 'canary',
            matched: false,
            reason: 'header x-canary = "1" (the request has no x-canary) does not hold',
          },
          { route_id: `${SITE_ID}-site`, name: 'site', matched: true },
        ],
      },
    })
  })
  return seen
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('routes take conditions and a request shows which route takes it', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto(`/sites/${SITE_ID}`)
  await page.getByRole('tab', { name: 'Routes' }).click()
  await expect(page.getByText('2 conditions')).toBeVisible()

  await page.getByRole('button', { name: 'Add route' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Path').fill('/api/')
  await sheet.getByRole('button', { name: 'Add a condition' }).click()
  await page.getByRole('menuitem', { name: 'Header' }).click()
  await sheet.getByLabel('Name', { exact: true }).last().fill('x-env')
  await sheet.getByLabel('Value').fill('staging')
  await sheet.getByRole('button', { name: 'Add a condition' }).click()
  await page.getByRole('menuitem', { name: 'None of' }).click()
  await sheet.getByRole('button', { name: 'Add inside' }).click()
  await page.getByRole('menuitem', { name: 'Client address' }).click()
  await sheet.getByLabel('Addresses or networks, separated by commas').fill('192.0.2.0/24')
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect.poll(() => seen.created.length).toBe(1)
  expect(seen.created[0]!.postDataJSON().match.conditions).toEqual([
    { kind: 'header', name: 'x-env', test: { op: 'equals', value: 'staging' } },
    { kind: 'not', condition: { kind: 'client', networks: ['192.0.2.0/24'] } },
  ])

  await page.getByRole('button', { name: 'Test a request' }).click()
  const tester = page.getByRole('dialog')
  await expect(tester.getByLabel('Host')).toHaveValue('shop.example')
  await tester.getByLabel('Path and query').fill('/api/items')
  await tester.getByLabel('Headers').fill('Accept: application/json')
  await tester.getByRole('button', { name: 'Test', exact: true }).click()
  await expect(tester.getByText("Taken by The site's own action")).toBeVisible()
  await expect(tester.getByText('(the request has no x-canary) does not hold')).toBeVisible()
  expect(seen.tested[0]!.postDataJSON()).toEqual({
    method: 'GET',
    host: 'shop.example',
    target: '/api/items',
    headers: [{ name: 'Accept', value: 'application/json' }],
    client: null,
  })
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})

test('static routes list directories, map media types and set cache headers', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto(`/sites/${SITE_ID}`)
  await page.getByRole('tab', { name: 'Routes' }).click()
  await page.getByRole('button', { name: 'Add route' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Path').fill('/files/')
  await sheet.getByRole('radio', { name: 'Static files' }).click()
  await sheet.getByLabel('Site directory').fill('files')
  await sheet.getByRole('combobox', { name: 'Directory listing' }).click()
  await page.getByRole('option', { name: 'Listed as JSON' }).click()
  await sheet.getByRole('button', { name: 'Add a media type' }).click()
  await sheet.getByLabel('Extension', { exact: true }).fill('wasm')
  const type = sheet.getByLabel('Media type', { exact: true })
  await type.fill('application')
  await expect(sheet.getByRole('alert')).toHaveText('Write type/subtype, such as application/wasm.')
  await expect(sheet.getByRole('button', { name: 'Create' })).toBeDisabled()
  await type.fill('application/wasm')
  await sheet.getByRole('button', { name: 'Add a cache rule' }).click()
  await page.getByRole('menuitem', { name: 'Hashed assets: a year' }).click()
  await sheet.getByRole('button', { name: 'Add a cache rule' }).click()
  await page.getByRole('menuitem', { name: 'Pages: revalidate' }).click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect.poll(() => seen.created.length).toBe(1)
  expect(seen.created[0]!.postDataJSON().action).toEqual({
    type: 'static',
    root: 'files',
    index_files: ['index.html'],
    spa_fallback: false,
    listing: 'json',
    media_types: { wasm: 'application/wasm' },
    cache: [
      {
        extensions: ['css', 'js', 'mjs', 'woff2', 'svg', 'png', 'jpg', 'webp', 'avif'],
        max_age_seconds: 31_536_000,
        immutable: true,
      },
      { extensions: ['html'] },
    ],
  })
})

test('a route stays out of its site cache or names a policy of its own', async ({ page }) => {
  const seen = await setUp(page)
  await page.route('**/api/v1/cache-policies', (handled) =>
    handled.fulfill({ json: [{ id: 'pages', used_by: [SITE_ID], etag: '"c1"' }] }),
  )
  await page.goto(`/sites/${SITE_ID}`)
  await page.getByRole('tab', { name: 'Routes' }).click()
  await page.getByRole('button', { name: 'Add route' }).first().click()
  const sheet = page.getByRole('dialog')
  await sheet.getByLabel('Path').fill('/live/')
  await sheet.getByRole('combobox', { name: 'Cache policy' }).click()
  await expect(page.getByRole('option', { name: 'pages' })).toBeVisible()
  await page.getByRole('option', { name: 'No caching' }).click()
  await sheet.getByRole('button', { name: 'Create' }).click()
  await expect.poll(() => seen.created.length).toBe(1)
  expect(seen.created[0]!.postDataJSON()).toMatchObject({ cache_policy_id: null, no_cache: true })
})
