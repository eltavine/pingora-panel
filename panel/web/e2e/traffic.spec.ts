import { expect, test, type Page } from '@playwright/test'
import { signIn } from './support'

const summary = {
  observed_at: '2026-10-04T10:00:00Z',
  window_seconds: 3600,
  requests: 1_200,
  requests_per_second: 0.33,
  statuses: {
    informational: 0,
    success: 1_100,
    redirection: 40,
    client_error: 48,
    server_error: 12,
  },
  latency: { p50: 0.012, p90: 0.08, p95: 0.25, p99: null },
  bytes_received: 2_048,
  bytes_sent: 1_572_864,
  open_connections: 3,
  tls_handshakes: 7,
  upstreams: [
    {
      upstream: 'app',
      requests: 900,
      error_ratio: 0.125,
      latency: { p50: null, p90: null, p95: 1.5, p99: null },
      connection_reuse_ratio: 0.875,
    },
  ],
  routes: [{ site: 'shop', route: 'checkout', requests: 600 }],
  domains: [{ site: 'shop', domain: '*.shop.example', requests: 450 }],
  upstream_failures: [
    {
      upstream: 'app',
      address: '10.0.0.7',
      port: 8080,
      error_type: 'connect_refused',
      failures: 12,
    },
  ],
  revision: 7,
  activated_at: '2026-10-04T09:00:00Z',
  lua: {
    runs: 480,
    failures: { timeout: 2, error: 1 },
    slow_runs: 5,
    latency: { p50: 0.0004, p90: 0.001, p95: 0.0031, p99: 0.02 },
    handlers: [
      {
        site: 'shop',
        route: 'checkout',
        phase: 'access',
        runs: 120,
        failures: 3,
        slow_runs: 5,
        p95: 0.012,
      },
    ],
    memory_bytes: 3_145_728,
  },
}

const series = {
  points: [0, 1, 2, 3].map((minute) => ({
    at: `2026-10-04T09:5${minute}:00Z`,
    requests_per_second: minute + 1,
    server_errors_per_second: minute === 2 ? 0.5 : 0,
    p95: minute === 1 ? null : 0.2,
  })),
}

async function setUp(page: Page) {
  await signIn(page)
  await page.addInitScript(() => {
    window.localStorage.setItem('pingora-panel.locale', 'en')
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route(/\/api\/v1\/sites(\?.*)?$/, (route) =>
    route.fulfill({ json: { items: [{ id: 'shop', name: 'Shop' }], next_cursor: null } }),
  )
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('the traffic of a window is summarized and charted', async ({ page }) => {
  await setUp(page)
  const windows: string[] = []
  await page.route(/\/api\/v1\/traffic(\?.*)?$/, (route) => {
    windows.push(new URL(route.request().url()).searchParams.get('window') ?? '')
    return route.fulfill({ json: summary })
  })
  await page.route(/\/api\/v1\/traffic\/series/, (route) => route.fulfill({ json: series }))

  await page.goto('/traffic')

  await expect(page.getByRole('heading', { name: 'Traffic' })).toBeVisible()
  await expect(page.getByText('1,200', { exact: true })).toBeVisible()
  await expect(page.getByText('250 ms', { exact: true }).first()).toBeVisible()
  await expect(page.getByText('1.5 MiB', { exact: true })).toBeVisible()
  await expect(page.getByRole('img', { name: 'Request rate' })).toBeVisible()
  await expect(page.getByRole('cell', { name: 'checkout' }).first()).toBeVisible()
  await expect(page.getByRole('cell', { name: '*.shop.example' })).toBeVisible()
  await expect(page.getByRole('cell', { name: '10.0.0.7:8080' })).toBeVisible()
  await expect(page.getByRole('cell', { name: '87.5%' })).toBeVisible()
  await expect(page.getByRole('cell', { name: '12.5%' })).toBeVisible()
  await expect(page.getByText('Active configuration revision #7')).toBeVisible()
  const lua = page.locator('[data-slot="card"]').filter({ hasText: 'Run time p95' })
  await expect(lua.getByText('3.10 ms', { exact: true })).toBeVisible()
  await expect(lua.getByText('timeout · 2')).toBeVisible()
  await expect(lua.getByRole('cell', { name: 'access' })).toBeVisible()
  await expect(lua.getByText('3 MiB', { exact: true })).toBeVisible()
  await expect(page.getByRole('img', { name: '4xx: 4% of requests' })).toBeVisible()
  expect(windows).toContain('3600')

  await page.getByRole('combobox', { name: 'Window' }).click()
  await page.getByRole('option', { name: '15 minutes' }).click()
  await expect.poll(() => windows.at(-1)).toBe('900')
  await expect(page).toHaveURL(/window=900/)

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})

test('a quiet window says so', async ({ page }) => {
  await setUp(page)
  await page.route(/\/api\/v1\/traffic(\?.*)?$/, (route) =>
    route.fulfill({
      json: {
        ...summary,
        requests: 0,
        upstreams: [],
        routes: [],
        domains: [],
        upstream_failures: [],
        revision: null,
      },
    }),
  )
  await page.route(/\/api\/v1\/traffic\/series/, (route) => route.fulfill({ json: { points: [] } }))

  await page.goto('/traffic')

  await expect(page.getByText('No traffic in this window')).toBeVisible()
})

test('an unavailable metrics backend can be retried', async ({ page }) => {
  await setUp(page)
  let available = false
  await page.route(/\/api\/v1\/traffic(\?.*)?$/, (route) =>
    available
      ? route.fulfill({ json: summary })
      : route.fulfill({
          status: 503,
          contentType: 'application/problem+json',
          json: {
            type: 'about:blank',
            title: 'Service Unavailable',
            status: 503,
            detail: 'Prometheus did not answer',
            code: 'UNAVAILABLE',
          },
        }),
  )
  await page.route(/\/api\/v1\/traffic\/series/, (route) => route.fulfill({ json: series }))

  await page.goto('/traffic')

  await expect(page.getByRole('alert')).toBeVisible()
  available = true
  await page.getByRole('button', { name: /retry/i }).click()
  await expect(page.getByText('1,200', { exact: true })).toBeVisible()
})
