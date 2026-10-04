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
    { capability: 'listeners', state: 'available', detail: '' },
    { capability: 'gateway_unit', state: 'available', detail: '' },
    { capability: 'containers', state: 'denied', detail: 'add the agent to the docker group' },
  ],
}

const unit = {
  name: 'pingora-panel-gatewayd.service',
  description: 'Pingora Panel gateway',
  load_state: 'loaded',
  active_state: 'active',
  sub_state: 'running',
  unit_file_state: 'enabled',
  main_pid: 1204,
  active_since: '2026-10-01T10:00:00Z',
  restarts: 0,
  result: 'success',
}

const listeners = {
  observed_at: '2026-10-04T10:00:00Z',
  listeners: [
    {
      address: '0.0.0.0',
      port: 80,
      uid: 0,
      processes: [{ pid: 912, name: 'nginx', executable: '/usr/sbin/nginx', uid: 0 }],
    },
    {
      address: '0.0.0.0',
      port: 443,
      uid: 0,
      processes: [{ pid: 1204, name: 'gatewayd', executable: '/usr/local/bin/gatewayd', uid: 0 }],
    },
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
  await page.route('**/api/v1/host/listeners', (route) => route.fulfill({ json: listeners }))
  await page.route('**/api/v1/host/gateway-unit', (route) => route.fulfill({ json: unit }))
  await page.route('**/api/v1/host/gateway-unit/*', (route) =>
    route.fulfill({ json: { ...unit, main_pid: 1301 } }),
  )
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
  const engine = capabilities.getByRole('listitem').filter({ hasText: 'Containers' })
  await expect(engine).toContainText('Missing a privilege')
  await expect(engine).toContainText('add the agent to the docker group')

  await expect(page.getByText('Ports 80 and 443')).toBeVisible()
  const web = page.getByRole('row').filter({ hasText: '/usr/sbin/nginx' })
  await expect(web).toContainText('PID 912')
  await expect(web).toContainText('Another process, the gateway cannot bind it')
  await expect(page.getByRole('row').filter({ hasText: 'gatewayd' })).toContainText('The gateway')

  await expect(page.getByText('Panel directories')).toBeVisible()
  await expect(page.getByRole('row').filter({ hasText: 'Configuration' })).toContainText('3 MiB')
  const logs = page.getByRole('row').filter({ hasText: '/var/log/pingora-panel' })
  await expect(logs).toContainText('Partial')
  await expect(logs).toContainText('2 entries unreadable')
  await expect(page.getByRole('row').filter({ hasText: 'Certificates' })).toContainText(
    'Does not exist',
  )
})

test('administrators restart the gateway service after confirming', async ({ page }) => {
  await setUp(page, host, ALL_PERMISSIONS, agent)
  await page.goto('/host')
  await expect(page.getByText('Active (running)')).toBeVisible()
  await expect(page.getByText('pingora-panel-gatewayd.service')).toBeVisible()
  const restarted = page.waitForRequest(
    (request) =>
      request.method() === 'POST' && request.url().endsWith('/api/v1/host/gateway-unit/restart'),
  )
  await page.getByRole('button', { name: 'Restart', exact: true }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Restart', exact: true }).click()
  await restarted
  await expect(page.getByText('Gateway service restarted')).toBeVisible()
})

test('the gateway service is shown without its actions to readers', async ({ page }) => {
  await setUp(
    page,
    host,
    ALL_PERMISSIONS.filter((permission) => permission !== 'host.manage'),
    agent,
  )
  await page.goto('/host')
  await expect(page.getByText('Active (running)')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Restart', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0)
})
