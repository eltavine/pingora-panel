import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const host = {
  observed_at: '2026-10-04T10:00:00Z',
  reporting: true,
  hostname: 'web-1',
  operating_system: 'Ubuntu 24.04.2 LTS',
  kernel_release: '6.8.0-41-generic',
  architecture: 'x86_64',
  host_time: '2026-10-04T10:00:00Z',
  time_zone: 'UTC',
  uptime_seconds: 90_000,
  cpu_count: 4,
  cpu_usage: 0.25,
  load1: 0.5,
  load5: 0.75,
  load15: 1,
  memory_total_bytes: 8 * 1024 ** 3,
  memory_available_bytes: 2 * 1024 ** 3,
  filesystems: [
    {
      mountpoint: '/var',
      device: '/dev/sdb1',
      fstype: 'ext4',
      size_bytes: 1024 ** 3,
      available_bytes: 0.03 * 1024 ** 3,
      used_ratio: 0.97,
      level: 'critical',
    },
    {
      mountpoint: '/',
      device: '/dev/sda1',
      fstype: 'ext4',
      size_bytes: 1024 ** 3,
      available_bytes: 0.5 * 1024 ** 3,
      used_ratio: 0.5,
      level: 'ok',
    },
  ],
  network_devices: [
    { device: 'eth0', receive_bytes_per_second: 2048, transmit_bytes_per_second: 512 },
  ],
}

const noAgent = { status: 'not_configured', build: null, hostname: null, capabilities: [] }

const agent = {
  status: 'connected',
  build: '0.1.0',
  hostname: 'web-1',
  capabilities: [
    { capability: 'directories', state: 'available', detail: '' },
    { capability: 'listeners', state: 'denied', detail: 'grant CAP_DAC_READ_SEARCH' },
  ],
}

const directories = {
  observed_at: '2026-10-04T10:00:00Z',
  directories: [
    {
      kind: 'configuration',
      path: '/var/lib/pingora-panel/gateway',
      present: true,
      bytes: 3 * 1024 ** 2,
      files: 148,
      unreadable: 0,
      truncated: false,
    },
    {
      kind: 'logs',
      path: '/var/log/pingora-panel',
      present: true,
      bytes: 2048,
      files: 3,
      unreadable: 2,
      truncated: true,
    },
    {
      kind: 'certificates',
      path: '/etc/pingora-panel/certificates',
      present: false,
      bytes: 0,
      files: 0,
      unreadable: 0,
      truncated: false,
    },
  ],
}

async function setUp(
  page: Page,
  body: object,
  permissions = ALL_PERMISSIONS,
  hostAgent: object = noAgent,
) {
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
    route.fulfill({ json: { version: 4, pending: false, applied_version: 4 } }),
  )
  await page.route('**/api/v1/host', (route) => route.fulfill({ json: body }))
  await page.route('**/api/v1/host/agent', (route) => route.fulfill({ json: hostAgent }))
  await page.route('**/api/v1/host/directories', (route) => route.fulfill({ json: directories }))
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('the host shows its figures and warns about full filesystems', async ({ page }) => {
  await setUp(page, host)
  await page.goto('/host')
  await expect(page.getByRole('heading', { level: 1, name: 'Host' })).toBeVisible()
  await expect(page.getByText('CPU, 4 cores')).toBeVisible()
  await expect(page.getByText('25%')).toBeVisible()
  await expect(page.getByText('75%')).toBeVisible()
  await expect(page.getByText('1 day 1 hour')).toBeVisible()
  await expect(page.getByText('Ubuntu 24.04.2 LTS')).toBeVisible()
  await expect(page.getByText('6.8.0-41-generic')).toBeVisible()
  const full = page.getByRole('row').filter({ hasText: '/var' })
  await expect(full.getByRole('status')).toHaveText('97% used, nearly full')
  await expect(full.getByRole('meter')).toHaveAttribute('aria-valuenow', '97')
  await expect(page.getByRole('row').filter({ hasText: 'eth0' })).toContainText('2 KiB/s')
})

test('a host without the node exporter says how to report', async ({ page }) => {
  await setUp(page, {
    observed_at: '2026-10-04T10:00:00Z',
    reporting: false,
    filesystems: [],
    network_devices: [],
  })
  await page.goto('/host')
  await expect(page.getByText('The host is not reporting')).toBeVisible()
})

test('the host page is offered only to accounts that read it', async ({ page }) => {
  await setUp(
    page,
    host,
    ALL_PERMISSIONS.filter((permission) => permission !== 'host.read'),
  )
  await page.goto('/')
  await expect(
    page.locator('[data-slot="sidebar"]').getByRole('link', { name: 'Host' }),
  ).toHaveCount(0)
})

test('a host without the agent says how to install it', async ({ page }) => {
  await setUp(page, host)
  await page.goto('/host')
  await expect(page.getByText('Not installed')).toBeVisible()
  await expect(page.getByText(/Install ops-agent on this host/)).toBeVisible()
  await expect(page.getByText('Panel directories')).toHaveCount(0)
})

test('the agent shows its capabilities and the space the panel takes', async ({ page }) => {
  await setUp(page, host, ALL_PERMISSIONS, agent)
  await page.goto('/host')
  await expect(page.getByText('Connected')).toBeVisible()
  await expect(page.getByText('Version 0.1.0')).toBeVisible()
  const capabilities = page.getByRole('list', { name: 'Capabilities' })
  await expect(
    capabilities.getByRole('listitem').filter({ hasText: 'Directory sizes' }),
  ).toContainText('Available')
  const listeners = capabilities.getByRole('listitem').filter({ hasText: 'Port diagnostics' })
  await expect(listeners).toContainText('Missing a privilege')
  await expect(listeners).toContainText('grant CAP_DAC_READ_SEARCH')

  await expect(page.getByText('Panel directories')).toBeVisible()
  await expect(page.getByRole('row').filter({ hasText: 'Configuration' })).toContainText('3 MiB')
  const logs = page.getByRole('row').filter({ hasText: '/var/log/pingora-panel' })
  await expect(logs).toContainText('Partial')
  await expect(logs).toContainText('2 entries unreadable')
  await expect(page.getByRole('row').filter({ hasText: 'Certificates' })).toContainText(
    'Does not exist',
  )
})
