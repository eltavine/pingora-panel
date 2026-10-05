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

const images = [
  {
    id: 'sha256:4f1c2a9be03d71aa6d2c',
    tags: ['nginx:1.27'],
    digests: ['nginx@sha256:9a8b'],
    created: '2026-09-04T08:00:00Z',
    size_bytes: 192 * 1024 ** 2,
    containers: 1,
    labels: { maintainer: 'NGINX' },
  },
  {
    id: 'sha256:0f9e8d7c6b5a49382716',
    tags: [],
    digests: [],
    created: '2026-08-04T08:00:00Z',
    size_bytes: 88 * 1024 ** 2,
    containers: 0,
    labels: {},
  },
]

const usage = {
  id: '4f1c2a9be03d71aa',
  name: 'shop-web-1',
  read_at: '2026-10-04T10:00:00.5Z',
  cpu_percent: 12.5,
  online_cpus: 4,
  memory_bytes: 200 * 1024 ** 2,
  memory_limit_bytes: 8 * 1024 ** 3,
  network: {
    received_bytes: 1_536,
    sent_bytes: 2_048,
    received_packets: 15,
    sent_packets: 20,
    errors: 1,
    dropped: 2,
  },
  block_read_bytes: 4_096,
  block_written_bytes: 8_192,
  pids: 5,
}

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
  await page.route(
    /\/api\/v1\/container-engines\/docker\/containers\/[\w.-]+(\/\w+)?(\?.*)?$/,
    (route) => {
      const request = route.request()
      if (request.method() === 'GET') {
        const found = containers[0]!
        return route.fulfill({
          json: {
            container: found,
            started_at: '2026-10-03T08:00:01Z',
            finished_at: null,
            exit_code: null,
            error: null,
            oom_killed: false,
            restarts: 2,
            health: 'healthy',
            restart_policy: 'unless-stopped',
            restart_retries: 0,
            hostname: 'web',
            user: null,
            working_directory: '/srv',
            platform: 'linux',
            mounts: [
              {
                kind: 'volume',
                name: 'shop_html',
                source: '/var/lib/docker/volumes/shop_html/_data',
                destination: '/usr/share/nginx/html',
                read_write: false,
              },
            ],
            networks: [
              {
                name: 'shop_default',
                ip_address: '172.18.0.2',
                ipv6_address: null,
                gateway: '172.18.0.1',
                mac_address: null,
                aliases: ['web'],
              },
            ],
          },
        })
      }
      const url = new URL(request.url())
      seen.changes.push(`${request.method()} ${url.pathname}${url.search}`)
      const [, , , , , , id, action] = url.pathname.split('/')
      const found = containers.find((container) => container.id === id)!
      const state = action === 'start' || action === 'restart' ? 'running' : 'exited'
      route.fulfill({
        json: {
          id: found.id,
          name: found.names[0],
          container: request.method() === 'DELETE' ? null : { ...found, state },
        },
      })
    },
  )
  await page.route(/\/api\/v1\/container-engines\/docker\/images(\?.*)?$/, (route) => {
    const search = new URL(route.request().url()).searchParams.get('search') ?? ''
    seen.queries.push(new URL(route.request().url()).searchParams)
    route.fulfill({
      json: {
        observed_at: '2026-10-04T10:00:00Z',
        images: images.filter((image) => image.tags.some((tag) => tag.includes(search)) || !search),
      },
    })
  })
  await page.route(/\/api\/v1\/container-engines\/docker\/images\/[^/?]+(\?.*)?$/, (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (request.method() === 'DELETE') {
      seen.changes.push(`DELETE ${url.pathname}${url.search}`)
      return route.fulfill({
        json: { id: images[1]!.id, untagged: [], deleted: [images[1]!.id] },
      })
    }
    return route.fulfill({
      json: {
        image: images[0],
        architecture: 'amd64',
        variant: null,
        os: 'linux',
        author: 'NGINX Docker Maintainers',
        comment: null,
        user: null,
        working_directory: '/',
        exposed_ports: ['80/tcp'],
        volumes: [],
        stop_signal: 'SIGQUIT',
        layers: 7,
      },
    })
  })
  await page.route(/\/api\/v1\/container-engines\/docker\/stats$/, (route) =>
    route.fulfill({ json: { observed_at: '2026-10-04T10:00:00Z', stats: [usage] } }),
  )
  await page.route(/\/api\/v1\/container-engines\/docker\/containers\/[\w.-]+\/stats$/, (route) =>
    route.fulfill({ json: usage }),
  )
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
  await expect(web).toContainText('12.5%')
  await expect(web).toContainText('200 MiB')
  await expect(web).toContainText('2.4%')
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
  await expect(page.getByRole('button', { name: 'Actions for shop-web-1' })).toHaveCount(0)
})

test('running containers are stopped after confirming and removed by force', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto('/containers')
  const actions = page.getByRole('button', { name: 'Actions for shop-web-1' })
  await actions.click()
  await expect(page.getByRole('menuitem', { name: 'Start', exact: true })).toHaveCount(0)
  await page.getByRole('menuitem', { name: 'Stop' }).click()
  const stop = page.getByRole('alertdialog', { name: 'Stop shop-web-1?' })
  await expect(stop).toContainText('kills it once its stop timeout passes')
  await stop.getByRole('button', { name: 'Stop' }).click()
  await expect(page.getByText('shop-web-1 stopped')).toBeVisible()

  await actions.click()
  await page.getByRole('menuitem', { name: 'Remove' }).click()
  const remove = page.getByRole('alertdialog', { name: 'Remove shop-web-1?' })
  await remove.getByRole('checkbox', { name: 'Kill it first if it is running' }).check()
  await remove.getByRole('button', { name: 'Remove' }).click()
  await expect(page.getByText('shop-web-1 removed')).toBeVisible()
  expect(seen.changes).toEqual([
    'POST /api/v1/container-engines/docker/containers/4f1c2a9be03d71aa/stop',
    'DELETE /api/v1/container-engines/docker/containers/4f1c2a9be03d71aa?force=true&volumes=false',
  ])
})

test('a container shows its labels, mounts and networks', async ({ page }) => {
  await setUp(page)
  await page.goto('/containers')
  await page.getByRole('button', { name: 'shop-web-1', exact: true }).click()
  const sheet = page.getByRole('dialog', { name: 'shop-web-1' })
  await expect(sheet).toContainText('Running · Healthy')
  await expect(sheet).toContainText('unless-stopped')
  await expect(sheet).toContainText('com.docker.compose.project')
  await expect(sheet).toContainText('/usr/share/nginx/html')
  await expect(sheet).toContainText('Read-only')
  await expect(sheet).toContainText('172.18.0.2')
  await expect(sheet).toContainText('12.5% · of 4 CPUs')
  await expect(sheet).toContainText('200 MiB / 8 GiB · 2.4%')
  await expect(sheet).toContainText('Errors 1 · dropped 2')
  await expect(sheet).toContainText('4 KiB read, 8 KiB written')
})

test('a stopped container starts without asking', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto('/containers')
  await page.getByRole('button', { name: 'Actions for nightly-report' }).click()
  await expect(page.getByRole('menuitem', { name: 'Stop' })).toHaveCount(0)
  await page.getByRole('menuitem', { name: 'Start', exact: true }).click()
  await expect(page.getByText('nightly-report started')).toBeVisible()
  expect(seen.changes).toEqual([
    'POST /api/v1/container-engines/docker/containers/e5d8b3a2f6c19d07/start',
  ])
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

const printed = [
  {
    time: '2026-10-04T10:00:01.000000001Z',
    stream: 'stdout',
    text: `${String.fromCharCode(27)}[32mGET${String.fromCharCode(27)}[0m / 200`,
  },
  { time: '2026-10-04T10:00:02Z', stream: 'stderr', text: 'upstream timed out' },
]

test("a container's logs are read, filtered and followed until it stops", async ({ page }) => {
  await setUp(page)
  const reads: URLSearchParams[] = []
  await page.route(/\/containers\/[\w.-]+\/logs(\?.*)?$/, (route) => {
    reads.push(new URL(route.request().url()).searchParams)
    route.fulfill({
      json: { observed_at: '2026-10-04T10:00:05Z', truncated: false, lines: printed },
    })
  })
  const tails: URL[] = []
  await page.routeWebSocket(/\/logs\/tail/, (socket) => {
    tails.push(new URL(socket.url()))
    socket.send(
      JSON.stringify({
        lines: [{ time: '2026-10-04T10:00:03Z', stream: 'stdout', text: 'GET /cart 200' }],
        cursor: '2026-10-04T10:00:03Z',
        error: null,
      }),
    )
    void socket.close({ code: 1000, reason: 'the container stopped' })
  })

  await page.goto('/containers')
  await page.getByRole('button', { name: 'Logs of shop-web-1' }).click()
  const sheet = page.getByRole('dialog', { name: 'shop-web-1 logs' })
  const log = sheet.getByRole('log')
  await expect(log).toContainText('GET / 200')
  await expect(log).toContainText('upstream timed out')
  await expect(sheet).toContainText('2 of 2 lines')
  expect(reads[0]?.get('lines')).toBe('200')
  expect(tails).toHaveLength(0)

  const filter = sheet.getByRole('searchbox', { name: 'Filter lines' })
  await filter.fill('TIMED')
  await expect(log).not.toContainText('GET / 200')
  await expect(sheet).toContainText('1 of 2 lines')
  await filter.fill('')

  await sheet.getByRole('button', { name: 'Follow live' }).click()
  await expect(log).toContainText('GET /cart 200')
  await expect(sheet).toContainText(
    'The container stopped; nothing more comes until it runs again.',
  )
  await expect(sheet.getByRole('button', { name: 'Follow live' })).toBeVisible()
  expect(tails[0]?.searchParams.get('after')).toBe('2026-10-04T10:00:02Z')

  const download = page.waitForEvent('download')
  await sheet.getByRole('button', { name: 'Download' }).click()
  expect((await download).suggestedFilename()).toMatch(/^shop-web-1-.+\.log$/)

  await sheet.getByRole('combobox', { name: 'Lines to read' }).click()
  await page.getByRole('option', { name: 'Last 1000 lines' }).click()
  await expect.poll(() => reads.at(-1)?.get('lines')).toBe('1000')
  await expect(log).not.toContainText('GET /cart 200')
})

test('logs are offered from a container and only to accounts that may read them', async ({
  page,
}) => {
  await setUp(page)
  await page.route(/\/containers\/[\w.-]+\/logs(\?.*)?$/, (route) =>
    route.fulfill({ json: { observed_at: null, truncated: false, lines: [] } }),
  )
  await page.goto('/containers')
  await page.getByRole('button', { name: 'shop-web-1', exact: true }).click()
  await page
    .getByRole('dialog', { name: 'shop-web-1' })
    .getByRole('button', { name: 'Logs' })
    .click()
  const sheet = page.getByRole('dialog', { name: 'shop-web-1 logs' })
  await expect(sheet.getByRole('log')).toContainText('It has printed nothing')

  await setUp(
    page,
    undefined,
    ALL_PERMISSIONS.filter((permission) => permission !== 'containers.inspect'),
  )
  await page.goto('/containers')
  await expect(page.getByRole('row').filter({ hasText: 'shop-web-1' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Logs of shop-web-1' })).toHaveCount(0)
})

test('images are listed, inspected and removed after confirming', async ({ page }) => {
  const seen = await setUp(page)
  await page.goto('/containers')
  await page.getByRole('tab', { name: 'Images' }).click()
  await expect(page).toHaveURL(/view=images/)
  const nginx = page.getByRole('row').filter({ hasText: 'nginx:1.27' })
  await expect(nginx).toContainText('192 MiB')
  await expect(nginx).toContainText('1 container')
  const dangling = page.getByRole('row').filter({ hasText: 'Nothing names it' })
  await expect(dangling).toContainText('0 containers')

  await nginx.getByRole('button', { name: 'nginx:1.27', exact: true }).click()
  const sheet = page.getByRole('dialog', { name: 'nginx:1.27' })
  await expect(sheet).toContainText('linux/amd64')
  await expect(sheet).toContainText('80/tcp')
  await expect(sheet).toContainText('NGINX Docker Maintainers')
  await page.keyboard.press('Escape')

  await page.getByRole('button', { name: 'Remove 0f9e8d7c6b5a' }).click()
  const dialog = page.getByRole('alertdialog', { name: 'Remove 0f9e8d7c6b5a?' })
  await dialog
    .getByRole('checkbox', {
      name: 'Remove it even if stopped containers use it or several tags name it',
    })
    .check()
  await dialog.getByRole('button', { name: 'Remove' }).click()
  await expect(page.getByText('0f9e8d7c6b5a removed')).toBeVisible()
  expect(seen.changes).toEqual([
    'DELETE /api/v1/container-engines/docker/images/sha256%3A0f9e8d7c6b5a49382716?force=true',
  ])

  await page.getByRole('searchbox', { name: 'Search images' }).fill('nginx')
  await expect.poll(() => seen.queries.at(-1)?.get('search')).toBe('nginx')
  await page.reload()
  await expect(page.getByRole('tab', { name: 'Images', selected: true })).toBeVisible()
})

test('readers see images without removing them', async ({ page }) => {
  await setUp(
    page,
    undefined,
    ALL_PERMISSIONS.filter((permission) => permission !== 'containers.manage'),
  )
  await page.goto('/containers?view=images')
  await expect(page.getByRole('row').filter({ hasText: 'nginx:1.27' })).toBeVisible()
  await expect(page.getByRole('button', { name: /^Remove / })).toHaveCount(0)
})
