import { expect, test, type Page } from '@playwright/test'
import { ALL_PERMISSIONS, signIn } from './support'

const docker = {
  id: 'docker',
  socket: '/run/docker.sock',
  enabled: true,
  reachable: true,
  detail: null,
  version: {
    version: '28.3.3',
    api_version: '1.51',
    os: 'linux',
    architecture: 'amd64',
    kernel_version: '6.8.0-41-generic',
    go_version: 'go1.24.5',
  },
  info: {
    name: 'web-1',
    operating_system: 'Ubuntu 24.04.2 LTS',
    containers: 3,
    running: 2,
    paused: 0,
    stopped: 1,
    images: 5,
    cpus: 4,
    memory_bytes: 8 * 1024 ** 3,
    storage_driver: 'overlay2',
    cgroup_driver: 'systemd',
  },
}

const podman = {
  id: 'podman',
  socket: '/run/podman/podman.sock',
  enabled: true,
  reachable: false,
  detail: 'the engine did not answer in time',
  version: null,
  info: null,
}

const containers = [
  {
    id: '4f1c2a9be03d71aa',
    names: ['shop-web-1'],
    image: 'nginx:1.27',
    image_id: 'sha256:4f1c',
    created: '2026-10-03T08:00:00Z',
    state: 'running',
    status: 'Up 26 hours',
    ports: [
      { private_port: 80, public_port: 8081, host_ip: '127.0.0.1', protocol: 'tcp' },
      { private_port: 443, public_port: null, host_ip: '', protocol: 'tcp' },
    ],
    labels: { 'com.docker.compose.project': 'shop' },
    compose_project: 'shop',
  },
  {
    id: 'e5d8b3a2f6c19d07',
    names: ['nightly-report'],
    image: 'ghcr.io/example/report:2.3',
    image_id: 'sha256:e5d8',
    created: '2026-10-04T02:00:00Z',
    state: 'exited',
    status: 'Exited (0) 7 hours ago',
    ports: [],
    labels: {},
    compose_project: null,
  },
]

interface Seen {
  queries: URLSearchParams[]
  changes: string[]
}

async function setUp(
  page: Page,
  engines: object | null = { engines: [docker, podman] },
  permissions = ALL_PERMISSIONS,
): Promise<Seen> {
  const seen: Seen = { queries: [], changes: [] }
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
  await page.route(/\/api\/v1\/container-engines$/, (route) =>
    engines
      ? route.fulfill({ json: engines })
      : route.fulfill({
          status: 422,
          contentType: 'application/problem+json',
          json: {
            type: 'about:blank',
            title: 'Unsupported capability',
            status: 422,
            code: 'UNSUPPORTED_CAPABILITY',
            detail: 'no host agent is configured',
          },
        }),
  )
  await page.route(/\/api\/v1\/container-engines\/docker\/containers(\?.*)?$/, (route) => {
    const query = new URL(route.request().url()).searchParams
    seen.queries.push(query)
    const search = query.get('search') ?? ''
    const states = (query.get('state') ?? '').split(',').filter(Boolean)
    route.fulfill({
      json: {
        observed_at: '2026-10-04T10:00:00Z',
        containers: containers.filter(
          (container) =>
            (!search || container.names[0]!.includes(search) || container.image.includes(search)) &&
            (states.length === 0 || states.includes(container.state)),
        ),
      },
    })
  })
  await page.route(/\/api\/v1\/container-engines\/\w+\/(enable|disable)$/, (route) => {
    const path = new URL(route.request().url()).pathname
    seen.changes.push(path)
    route.fulfill({ json: { ...docker, enabled: path.endsWith('/enable') } })
  })
  return seen
}

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(
    () => (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [],
  )
  expect(violations).toEqual([])
})

test('engines show how they are and containers what they publish', async ({ page }) => {
  await setUp(page)
  await page.goto('/containers')
  await expect(page.getByRole('heading', { level: 1, name: 'Containers' })).toBeVisible()
  const engines = page.getByRole('region', { name: 'Container engines' })
  await expect(engines).toContainText('Reachable, version 28.3.3 (API 1.51)')
  await expect(engines).toContainText('2 of 3 running')
  await expect(engines).toContainText('the engine did not answer in time')

  const web = page.getByRole('row').filter({ hasText: 'shop-web-1' })
  await expect(web).toContainText('127.0.0.1:8081->80/tcp')
  await expect(web).toContainText('443/tcp')
  await expect(web).toContainText('Compose: shop')
  await expect(web.getByRole('status')).toHaveText('Running')
  await expect(page.getByRole('row').filter({ hasText: 'nightly-report' })).toContainText(
    'Exited (0) 7 hours ago',
  )
})

test('containers are searched and filtered by state from the address bar', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto('/containers')
  await page.getByRole('searchbox', { name: 'Search' }).fill('nginx')
  await expect(page).toHaveURL(/search=nginx/)
  await expect(page.getByRole('row').filter({ hasText: 'nightly-report' })).toHaveCount(0)

  await page.getByRole('combobox', { name: 'State' }).click()
  await page.getByRole('option', { name: 'Exited' }).click()
  await expect(page).toHaveURL(/state=exited/)
  await expect(page.getByText('No containers match')).toBeVisible()
  expect(seen.queries.at(-1)?.get('search')).toBe('nginx')
  expect(seen.queries.at(-1)?.get('state')).toBe('exited')

  await page.goto('/containers?state=exited')
  await expect(page.getByRole('row').filter({ hasText: 'nightly-report' })).toBeVisible()
})

test('disabling an engine asks first', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto('/containers')
  await expect(page.getByText("Reaching an engine's socket is root on this host.")).toBeVisible()
  const card = page.locator('[data-slot="card"]').filter({ hasText: '/run/docker.sock' })
  await card.getByRole('button', { name: 'Disable' }).click()
  const dialog = page.getByRole('alertdialog', { name: 'Disable Docker?' })
  await expect(dialog).toContainText('The containers keep running.')
  await dialog.getByRole('button', { name: 'Disable' }).click()
  await expect(page.getByText('Docker disabled')).toBeVisible()
  expect(seen.changes).toEqual(['/api/v1/container-engines/docker/disable'])
})

test('a disabled engine is left alone until it is enabled', async ({ page }) => {
  await setUp(page, { engines: [{ ...docker, enabled: false, version: null, info: null }] })
  await page.goto('/containers')
  await expect(page.getByText('Enable Docker to see what runs on it.')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Enable' })).toBeVisible()
})

test('readers see engines and containers without changing them', async ({ page }) => {
  await setUp(
    page,
    undefined,
    ALL_PERMISSIONS.filter((permission) => permission !== 'containers.manage'),
  )
  await page.goto('/containers')
  await expect(page.getByRole('row').filter({ hasText: 'shop-web-1' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Disable' })).toHaveCount(0)
  await expect(page.getByText("Reaching an engine's socket is root on this host.")).toHaveCount(0)
})

test('a host without the agent says how to manage containers', async ({ page }) => {
  await setUp(page, null)
  await page.goto('/containers')
  await expect(page.getByText('No host agent manages containers here')).toBeVisible()
})

test('the containers page is offered only to accounts that read them', async ({ page }) => {
  await setUp(
    page,
    undefined,
    ALL_PERMISSIONS.filter((permission) => permission !== 'containers.read'),
  )
  await page.goto('/')
  await expect(
    page.locator('[data-slot="sidebar"]').getByRole('link', { name: 'Containers' }),
  ).toHaveCount(0)
})
