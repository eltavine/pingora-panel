import { expect, test, type Page, type Request } from '@playwright/test'
import { signIn } from './support'

const ACTIVE_HASH = 'a'.repeat(64)
const NEXT_HASH = 'b'.repeat(64)

const readyStatus = {
  ready: true,
  active_revision_id: 6,
  active_hash: ACTIVE_HASH,
  prepared_count: 0,
  adapter_version: 'pingora-v1',
  schema_version: 'v1',
  message: null,
}

async function useEnglish(page: Page) {
  await page.addInitScript(() => window.localStorage.setItem('pingora-panel.locale', 'en'))
}

async function mockStatus(page: Page, body: object = readyStatus) {
  await page.route('**/api/v1/gateway/status', (route) => route.fulfill({ json: body }))
}

async function expectNoHorizontalOverflow(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
}

test.beforeEach(async ({ page }) => {
  await signIn(page)
  await useEnglish(page)
  // The preview server sends the production Content Security Policy; any
  // violation means the console would break when served by the API.
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

test('the overview reports readiness and the active configuration', async ({ page }) => {
  await mockStatus(page)
  await page.route('**/api/v1/gateway/data-plane', (route) =>
    route.fulfill({
      json: {
        generation: 3,
        worker_count: 4,
        uptime_seconds: 3_720,
        gateway_version: '0.2.0',
        engine_version: '0.9.0',
        adapter_version: 'pingora-v1',
        listeners: [{ id: 'public', address: '0.0.0.0:443', tls: true, http1: true, http2: true }],
      },
    }),
  )
  await page.goto('/')

  await expect(page.getByRole('heading', { name: 'Gateway overview' })).toBeVisible()
  await expect(page.getByRole('status').filter({ hasText: 'Ready' })).toBeVisible()
  await expect(page.getByText('pingora-v1').first()).toBeVisible()
  await expect(page.getByTitle(ACTIVE_HASH)).toBeVisible()
  await expect(page.getByText('1h 2m')).toBeVisible()
  await expect(page.getByLabel('Workers')).toHaveValue('4')
  await expect(page.getByText('0.0.0.0:443')).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('the overview lists the control-plane modules running', async ({ page }) => {
  await mockStatus(page)
  await page.route('**/api/v1/platform/services', (route) =>
    route.fulfill({
      json: {
        observed_at: '2026-10-08T10:00:00Z',
        services: [
          {
            service: 'config-service',
            instance_id: '0192a4e5-77aa-7c1b-9f00-3c4d5e6f7a8b',
            build_version: '0.3.0',
            started_at: '2026-10-08T08:00:00Z',
            schema_version: null,
            protocols: [{ name: 'pingora.panel', min_revision: 1, max_revision: 4 }],
            capabilities: [{ name: 'configuration', version: '1' }],
          },
        ],
      },
    }),
  )
  await page.goto('/')

  await expect(page.getByText('Control-plane services')).toBeVisible()
  await expect(page.getByText('config-service', { exact: true })).toBeVisible()
  await expect(page.getByText('pingora.panel 1–4')).toBeVisible()
  await expect(page.getByText('Offers configuration')).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('the overview points at failed requests, failing upstreams and certificates', async ({
  page,
}) => {
  await mockStatus(page)
  await page.route('**/api/v1/gateway/data-plane', (route) =>
    route.fulfill({ json: { generation: 1, worker_count: 2, listeners: [] } }),
  )
  await page.route(/\/api\/v1\/logs\?/, (route) =>
    route.fulfill({
      json: {
        records: [
          {
            time: '2026-10-04T10:00:00Z',
            kind: 'access',
            line: '{}',
            site: 'shop',
            status: 502,
            method: 'GET',
            path: '/cart',
            fields: {},
          },
        ],
        next_until: null,
      },
    }),
  )
  await page.route(/\/api\/v1\/traffic\?/, (route) =>
    route.fulfill({
      json: {
        window_seconds: 900,
        requests: 0,
        requests_per_second: 0,
        statuses: {},
        latency: {},
        upstreams: [],
        routes: [],
        domains: [],
        upstream_failures: [
          {
            upstream: 'app',
            address: '10.0.0.7',
            port: 8080,
            error_type: 'connect_refused',
            failures: 12,
          },
        ],
      },
    }),
  )
  await page.route('**/api/v1/certificates', (route) =>
    route.fulfill({
      json: [
        {
          id: 'shop',
          names: ['shop.example'],
          status: 'expired',
          not_after: '2026-09-30T00:00:00Z',
        },
        { id: 'docs', names: ['docs.example'], status: 'valid', not_after: '2027-09-30T00:00:00Z' },
      ],
    }),
  )
  await page.route('**/api/v1/acme-certificates', (route) =>
    route.fulfill({
      json: [
        {
          id: 'blog',
          names: ['blog.example'],
          state: 'failing',
          failures: 3,
          last_error: {
            at: '2026-10-04T08:00:00Z',
            code: 'rejected',
            message: 'the CA refused the order',
          },
        },
      ],
    }),
  )
  await page.goto('/')

  const failed = page.getByRole('region', { name: 'Failed requests' })
  await expect(failed).toContainText('502 · shop')
  await expect(failed).toContainText('GET /cart')
  await expect(failed.getByRole('link', { name: 'Show failed requests' })).toHaveAttribute(
    'href',
    '/logs?status=5xx',
  )
  const upstreams = page.getByRole('region', { name: 'Upstream failures' })
  await expect(upstreams).toContainText('app · 10.0.0.7:8080')
  await expect(upstreams).toContainText('connect_refused: 12 failed attempts')
  const certificates = page.getByRole('region', { name: 'Certificate problems' })
  await expect(certificates).toContainText('blog.example')
  await expect(certificates).toContainText('the CA refused the order')
  await expect(certificates).toContainText('shop.example')
  await expect(certificates).toContainText('Expired')
  await expect(certificates).not.toContainText('docs.example')
  await expectNoHorizontalOverflow(page)
})

test('file checks point at exposed keys and links out of static roots', async ({ page }) => {
  await mockStatus(page)
  await page.route('**/api/v1/gateway/data-plane', (route) =>
    route.fulfill({ json: { generation: 1, worker_count: 2, listeners: [] } }),
  )
  await page.route('**/api/v1/gateway/file-checks', (route) =>
    route.fulfill({
      json: {
        checked_at: '2026-10-03T08:00:00.000Z',
        active_revision_id: 6,
        private_keys: [
          { file: 'edge.key', tls_profile_ids: ['edge'], mode: '0600', owner_only: true },
          { file: 'legacy.key', tls_profile_ids: ['legacy'], mode: '0644', owner_only: false },
        ],
        static_roots: [
          {
            id: 'docs',
            root: 'docs',
            inside: true,
            escaping_links: [{ path: 'old/etc', target: '/etc' }],
            entries_checked: 12,
            truncated: false,
          },
        ],
      },
    }),
  )
  await page.goto('/')

  await expect(page.getByText('2 problems')).toBeVisible()
  await expect(page.getByText('Readable by others')).toBeVisible()
  await expect(page.getByText('Owner only')).toBeVisible()
  await expect(page.getByText('old/etc → /etc')).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('an unreachable API is reported with a retry action', async ({ page }) => {
  await page.route('**/api/v1/gateway/status', (route) => route.abort('connectionrefused'))
  await page.goto('/')

  const alert = page.getByRole('alert')
  await expect(alert).toContainText('The management API is unreachable')
  await expect(alert.getByRole('button', { name: 'Retry' })).toBeVisible()
  await expectNoHorizontalOverflow(page)
})

test('the color scheme can be switched and persists', async ({ page }) => {
  await mockStatus(page)
  await page.goto('/')

  await page.getByRole('button', { name: 'Appearance' }).click()
  await page.getByRole('menuitem', { name: 'Dark' }).click()
  await expect(page.locator('html')).toHaveClass(/dark/)

  await page.reload()
  await expect(page.locator('html')).toHaveClass(/dark/)
})

test('a snapshot is validated, prepared and activated with compare-and-swap', async ({ page }) => {
  await mockStatus(page)
  const commands: Request[] = []
  await page.route('**/api/v1/gateway/validate', (route) =>
    route.fulfill({ json: { valid: true, diagnostics: [] } }),
  )
  await page.route('**/api/v1/gateway/prepare', (route) => {
    commands.push(route.request())
    return route.fulfill({
      json: { prepare_token: 'prepare-7', revision_id: 7, content_hash: NEXT_HASH },
    })
  })
  await page.route('**/api/v1/gateway/activate', (route) => {
    commands.push(route.request())
    return route.fulfill({
      json: { revision_id: 7, content_hash: NEXT_HASH, previous_active_hash: ACTIVE_HASH },
    })
  })

  await page.goto('/publish')
  await page.getByLabel('Snapshot document').fill('{"sites":[],"routes":[]}')
  await page.getByRole('button', { name: 'Validate' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Validation passed' })).toBeVisible()

  await page.getByRole('button', { name: 'Prepare' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Snapshot prepared' })).toBeVisible()
  await expect(page.getByLabel('Expected current hash')).toHaveValue(ACTIVE_HASH)

  await page.getByRole('button', { name: 'Activate' }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog).toContainText('Revision 7 atomically replaces the active configuration.')
  await dialog.getByRole('button', { name: 'Activate' }).click()
  await expect(page.getByText('Revision 7 is active')).toBeVisible()

  const [prepare, activate] = commands
  for (const request of [prepare, activate]) {
    const headers = request!.headers()
    expect(headers['idempotency-key']).toMatch(/^[\x21-\x7e]+$/)
    expect(Date.parse(headers['x-deadline']!)).toBeGreaterThan(Date.now())
  }
  expect(activate!.postDataJSON()).toEqual({
    prepare_token: 'prepare-7',
    expected_active_hash: ACTIVE_HASH,
  })
  await expectNoHorizontalOverflow(page)
})

test('receipts start from an explanatory empty state', async ({ page }) => {
  await page.goto('/receipts')

  await expect(page.getByText('Enter an idempotency key')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Look up' })).toBeDisabled()
  await expectNoHorizontalOverflow(page)
})
