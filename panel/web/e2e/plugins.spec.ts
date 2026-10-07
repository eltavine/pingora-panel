import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const PORTS = ['dns01', 'secrets', 'notifications']

function version(number: string) {
  return {
    version: number,
    publisher: 'Acme',
    description: 'DNS records at Acme',
    ports: PORTS,
    capabilities: [...PORTS, 'secret-references'],
    protocol_versions: [1],
    compatible: true,
    signed_by: 'acme',
    problems: [],
    config_schema: {
      type: 'object',
      properties: {
        zone: { type: 'string', title: 'Zone' },
        token: { type: 'string', format: 'secret-reference', title: 'API token' },
        ttl: { type: 'integer', title: 'Record TTL' },
      },
    },
    resources: {
      memory_bytes: 0,
      cpu_seconds: 0,
      open_files: 256,
      concurrency: 8,
      call_timeout_ms: 0,
    },
    executable_sha256: 'ab'.repeat(32),
  }
}

const plugin = {
  name: 'acme-dns',
  state: 'enabled',
  active_version: '1.0.0',
  versions: [version('1.0.0'), version('1.1.0')],
  grants: ['dns01'],
  settings: { zone: 'example.com', ttl: 60 },
  limits: { memory_bytes: 0, cpu_seconds: 0, open_files: 0, concurrency: 0, call_timeout_ms: 0 },
  effective_limits: {
    memory_bytes: 536870912,
    cpu_seconds: 0,
    open_files: 256,
    concurrency: 8,
    call_timeout_ms: 10000,
  },
  health: {
    status: 'serving',
    version: '1.0.0',
    started_at: '2026-10-07T10:00:00Z',
    checked_at: '2026-10-07T10:05:00Z',
    failures: 0,
    restarts: 1,
  },
  updated_at: '2026-10-07T10:00:00Z',
  etag: '"7"',
}

/** What the console asked for. */
interface Seen {
  discovered: number
  keys: unknown[]
  secrets: { path: string; body: unknown }[]
  changes: { path: string; body: unknown; ifMatch?: string }[]
}

async function setUp(page: Page, permissions = ALL_PERMISSIONS): Promise<Seen> {
  const seen: Seen = { discovered: 0, keys: [], secrets: [], changes: [] }
  await signIn(page, permissions)
  await page.addInitScript(() => {
    window.localStorage.setItem('pingora-panel.locale', 'en')
    const violations: string[] = []
    Object.assign(window, { __cspViolations: violations })
    document.addEventListener('securitypolicyviolation', (event) =>
      violations.push(`${event.violatedDirective} ${event.blockedURI}`),
    )
  })
  await page.route('**/api/v1/config/draft', (route) =>
    route.fulfill({ json: { version: 6, pending: false, applied_version: 6 } }),
  )
  const list = {
    protocol_versions: [1],
    ports: PORTS,
    capabilities: [...PORTS, 'secret-references'],
    limits_enforced: true,
    discovered_at: '2026-10-07T10:00:00Z',
    plugins: [plugin],
  }
  await page.route('**/api/v1/plugins', (route) => route.fulfill({ json: list }))
  await page.route('**/api/v1/plugins/discover', (route) => {
    seen.discovered += 1
    return route.fulfill({ json: list })
  })
  await page.route('**/api/v1/plugins/acme-dns', (route) =>
    route.fulfill({ json: plugin, headers: { etag: plugin.etag } }),
  )
  await page.route('**/api/v1/plugins/acme-dns/*', (route) => {
    const request = route.request()
    seen.changes.push({
      path: new URL(request.url()).pathname,
      body: request.postDataJSON(),
      ifMatch: request.headers()['if-match'],
    })
    return route.fulfill({ json: { ...plugin, etag: '"8"' }, headers: { etag: '"8"' } })
  })
  await page.route('**/api/v1/plugin-keys', (route) => {
    if (route.request().method() === 'POST') {
      const body = route.request().postDataJSON()
      seen.keys.push(body)
      return route.fulfill({
        status: 201,
        json: { ...body, key_id: '0123456789ABCDEF', created_at: '2026-10-07T10:00:00Z' },
      })
    }
    return route.fulfill({
      json: [
        {
          id: 'acme',
          key_id: '9F3C1A7E5B2D4C60',
          public_key: 'RWQ',
          comment: 'Acme releases',
          created_at: '2026-10-06T10:00:00Z',
        },
      ],
    })
  })
  await page.route('**/api/v1/plugin-secrets', (route) =>
    route.fulfill({ json: [{ name: 'dns-token', updated_at: '2026-10-06T10:00:00Z' }] }),
  )
  await page.route('**/api/v1/plugin-secrets/*', (route) => {
    seen.secrets.push({
      path: new URL(route.request().url()).pathname,
      body: route.request().postDataJSON(),
    })
    return route.fulfill({ json: { name: 'acme-token', updated_at: '2026-10-07T10:00:00Z' } })
  })
  return seen
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('plugins are discovered, trusted, granted, configured, limited and upgraded', async ({
  page,
}) => {
  const seen = await setUp(page)
  await page.goto('/plugins')
  await expect(page.getByRole('heading', { name: 'Plugins', level: 1 })).toBeVisible()
  await expect(page.getByRole('link', { name: /acme-dns/ }).first()).toBeVisible()
  await expect(page.getByText('Running').first()).toBeVisible()
  await expect(page.getByText('Resource limits enforced')).toBeVisible()
  await expect(page.getByText('9F3C1A7E5B2D4C60')).toBeVisible()

  await page.getByRole('button', { name: 'Discover' }).click()
  await expect(page.getByText('Read the plugins directory again')).toBeVisible()
  expect(seen.discovered).toBe(1)

  await page.getByRole('button', { name: 'Trust a key' }).click()
  const trust = page.getByRole('alertdialog')
  await trust.getByLabel('ID', { exact: true }).fill('acme-2')
  await trust.getByLabel('Public key').fill('untrusted comment: minisign public key\nRWQ2')
  await trust.getByRole('button', { name: 'Trust a key' }).click()
  await expect(page.getByText('Trusted the key acme-2')).toBeVisible()
  expect(seen.keys).toEqual([
    { id: 'acme-2', public_key: 'untrusted comment: minisign public key\nRWQ2', comment: '' },
  ])

  await page.getByRole('button', { name: 'Keep a secret' }).click()
  const keep = page.getByRole('alertdialog')
  await keep.getByLabel('Name').fill('acme-token')
  await keep.getByLabel('Value').fill('s3cret')
  await keep.getByRole('button', { name: 'Keep a secret' }).click()
  await expect(page.getByText('Kept the secret acme-token')).toBeVisible()
  expect(seen.secrets).toEqual([
    { path: '/api/v1/plugin-secrets/acme-token', body: { value: 's3cret' } },
  ])

  await page.getByRole('link', { name: 'Open acme-dns' }).click()
  await expect(page.getByRole('heading', { name: 'acme-dns', level: 1 })).toBeVisible()
  await expect(page.getByText('Serving')).toBeVisible()

  await page.getByRole('checkbox', { name: /notifications/ }).click()
  await page.getByRole('button', { name: 'Save grants' }).click()
  await expect(page.getByText('Saved the grants')).toBeVisible()

  await page.getByLabel('API token').fill('vault:dns-token')
  await page.getByLabel('Record TTL').fill('300')
  await page.getByRole('button', { name: 'Save settings' }).click()
  await expect(page.getByText('Saved the settings')).toBeVisible()

  await page.getByLabel('Calls at once').fill('4')
  await page.getByRole('button', { name: 'Save limits' }).click()
  await expect(page.getByText('Saved the resource limits')).toBeVisible()

  await page.getByRole('button', { name: 'Upgrade' }).click()
  await page.getByRole('menuitem', { name: 'Upgrade to 1.1.0' }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Upgrade' }).click()
  await expect(page.getByText('Upgraded to 1.1.0')).toBeVisible()

  expect(seen.changes).toEqual([
    {
      path: '/api/v1/plugins/acme-dns/grants',
      body: { capabilities: ['dns01', 'notifications'] },
      ifMatch: '"7"',
    },
    {
      path: '/api/v1/plugins/acme-dns/settings',
      body: { zone: 'example.com', token: 'vault:dns-token', ttl: 300 },
      ifMatch: '"7"',
    },
    {
      path: '/api/v1/plugins/acme-dns/limits',
      body: { memory_bytes: 0, cpu_seconds: 0, open_files: 0, concurrency: 4, call_timeout_ms: 0 },
      ifMatch: '"7"',
    },
    { path: '/api/v1/plugins/acme-dns/upgrade', body: { version: '1.1.0' }, ifMatch: '"7"' },
  ])

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  )
  expect(overflow).toBeLessThanOrEqual(0)
})

test('accounts that only read plugins cannot change them', async ({ page }) => {
  await setUp(page, ['plugins.read'])
  await page.goto('/plugins')
  await expect(page.getByRole('link', { name: /acme-dns/ }).first()).toBeVisible()
  await expect(page.getByRole('button', { name: 'Discover' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Trust a key' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Keep a secret' })).toHaveCount(0)

  await page.goto('/plugins/acme-dns')
  await expect(page.getByRole('heading', { name: 'acme-dns', level: 1 })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Disable' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Save settings' })).toHaveCount(0)
  await expect(page.getByRole('checkbox', { name: /notifications/ })).toBeDisabled()
})
