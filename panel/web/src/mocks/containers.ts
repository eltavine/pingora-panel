import { http, HttpResponse, ws, type AnyHandler } from 'msw'
import type {
  ContainerEngineView,
  ContainerLogLineView,
  ContainerLogsView,
  ContainerLogTailMessage,
  ContainerStatsListView,
  ContainerStatsView,
  ContainerView,
  EngineDiskUsageView,
  EngineNetworkListView,
  EngineVolumeListView,
  ImageDetailView,
  ImageListView,
  ImageView,
} from '@/api/generated'

const GIB = 1024 ** 3
/** How far apart the lines a container prints are. */
const PRINT_INTERVAL_MS = 2_000
const PATHS = ['/', '/cart', '/api/orders', '/static/app.js', '/healthz']

/** The `index`th line a web container prints, coloured as a terminal shows it. */
function printed(at: number, index: number): ContainerLogLineView {
  const time = new Date(at).toISOString()
  if (index % 7 === 0) {
    return {
      time,
      stream: 'stderr',
      text: `[warn] upstream answered in ${120 + (index % 50)} ms, over its budget`,
    }
  }
  const path = PATHS[index % PATHS.length]
  return {
    time,
    stream: 'stdout',
    text: `\u001b[32m200\u001b[0m GET ${path} ${3 + (index % 40)} ms`,
  }
}

function hoursAgo(hours: number): string {
  return new Date(Date.now() - hours * 3_600_000).toISOString()
}

/** Docker running a small Compose project, and a Podman socket the panel leaves alone. */
export function containerHandlers(): AnyHandler[] {
  const engines: ContainerEngineView[] = [
    {
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
        name: 'edge-1',
        operating_system: 'Ubuntu 24.04.2 LTS',
        containers: 4,
        running: 2,
        paused: 1,
        stopped: 1,
        images: 7,
        cpus: 4,
        memory_bytes: 8 * GIB,
        storage_driver: 'overlay2',
        cgroup_driver: 'systemd',
      },
    },
    {
      id: 'podman',
      socket: '/run/podman/podman.sock',
      enabled: false,
      reachable: false,
      detail: null,
      version: null,
      info: null,
    },
  ]
  const containers: ContainerView[] = [
    {
      id: '4f1c2a9be03d71aa',
      names: ['shop-web-1'],
      image: 'nginx:1.27',
      image_id: 'sha256:4f1c2a9b',
      created: hoursAgo(26),
      state: 'running',
      status: 'Up 26 hours',
      ports: [
        { private_port: 80, public_port: 8081, host_ip: '127.0.0.1', protocol: 'tcp' },
        { private_port: 443, public_port: null, host_ip: '', protocol: 'tcp' },
      ],
      labels: { 'com.docker.compose.project': 'shop', 'com.docker.compose.service': 'web' },
      compose_project: 'shop',
    },
    {
      id: '9b2e7d40c1f8a3e2',
      names: ['shop-db-1'],
      image: 'postgres:17',
      image_id: 'sha256:9b2e7d40',
      created: hoursAgo(26),
      state: 'running',
      status: 'Up 26 hours (healthy)',
      ports: [{ private_port: 5432, public_port: null, host_ip: '', protocol: 'tcp' }],
      labels: { 'com.docker.compose.project': 'shop', 'com.docker.compose.service': 'db' },
      compose_project: 'shop',
    },
    {
      id: 'c07a5e1d9f2b6c48',
      names: ['cache'],
      image: 'redis:7.4',
      image_id: 'sha256:c07a5e1d',
      created: hoursAgo(50),
      state: 'paused',
      status: 'Up 2 days (Paused)',
      ports: [],
      labels: {},
      compose_project: null,
    },
    {
      id: 'e5d8b3a2f6c19d07',
      names: ['nightly-report'],
      image: 'ghcr.io/example/report:2.3',
      image_id: 'sha256:e5d8b3a2',
      created: hoursAgo(8),
      state: 'exited',
      status: 'Exited (0) 7 hours ago',
      ports: [],
      labels: {},
      compose_project: null,
    },
  ]

  /** What a running container uses, varying a little with the time. */
  function usage(container: ContainerView): ContainerStatsView {
    const wave = (Math.sin(Date.now() / 60_000 + container.id.length) + 1) / 2
    const memory = (64 + container.id.length * 12) * 1024 ** 2
    return {
      id: container.id,
      name: container.names[0] ?? container.id,
      read_at: new Date().toISOString(),
      cpu_percent: 2 + wave * 40,
      online_cpus: 4,
      memory_bytes: Math.round(memory * (0.8 + wave * 0.2)),
      memory_limit_bytes: 8 * GIB,
      network: {
        received_bytes: 48 * 1024 ** 2,
        sent_bytes: 12 * 1024 ** 2,
        received_packets: 52_000,
        sent_packets: 31_000,
        errors: 0,
        dropped: 3,
      },
      block_read_bytes: 210 * 1024 ** 2,
      block_written_bytes: 36 * 1024 ** 2,
      pids: 4 + (container.id.length % 9),
    }
  }

  const images: ImageView[] = [
    {
      id: 'sha256:4f1c2a9be03d71aa6d2c1e8f4b7a9c3d',
      tags: ['nginx:1.27'],
      digests: ['nginx@sha256:9a8b7c6d5e4f'],
      created: hoursAgo(24 * 30),
      size_bytes: 192 * 1024 ** 2,
      containers: 1,
      labels: { maintainer: 'NGINX Docker Maintainers' },
    },
    {
      id: 'sha256:a1b2c3d4e5f60718293a4b5c6d7e8f90',
      tags: ['redis:7.4', 'redis:7'],
      digests: [],
      created: hoursAgo(24 * 12),
      size_bytes: 117 * 1024 ** 2,
      containers: 1,
      labels: {},
    },
    {
      id: 'sha256:0f9e8d7c6b5a49382716051f2e3d4c5b',
      tags: [],
      digests: [],
      created: hoursAgo(24 * 60),
      size_bytes: 88 * 1024 ** 2,
      containers: 0,
      labels: {},
    },
  ]

  const tail = ws.link(/\/api\/v1\/container-engines\/[^/]+\/containers\/[^/]+\/logs\/tail/)

  function missing(container: string) {
    return HttpResponse.json(
      {
        type: 'about:blank',
        title: 'Not found',
        status: 404,
        code: 'NOT_FOUND',
        detail: `No such container: ${container}`,
      },
      { status: 404 },
    )
  }

  return [
    http.get('*/api/v1/container-engines', () => HttpResponse.json({ engines })),
    http.post<{ engine: string; action: string }>(
      '*/api/v1/container-engines/:engine/:action',
      ({ params }) => {
        const engine = engines.find((candidate) => candidate.id === params.engine)
        if (!engine) {
          return HttpResponse.json(
            { type: 'about:blank', title: 'Not found', status: 404, code: 'NOT_FOUND' },
            { status: 404 },
          )
        }
        engine.enabled = params.action === 'enable'
        return HttpResponse.json(engine)
      },
    ),
    http.get<{ engine: string; container: string }>(
      '*/api/v1/container-engines/:engine/containers/:container',
      ({ params }) => {
        const found = containers.find((container) => container.id === params.container)
        if (!found) {
          return missing(params.container)
        }
        const running = found.state === 'running' || found.state === 'paused'
        return HttpResponse.json({
          container: found,
          started_at: found.created,
          finished_at: running ? null : hoursAgo(7),
          exit_code: running ? null : 0,
          error: null,
          oom_killed: false,
          restarts: 0,
          health: found.status.includes('(healthy)') ? 'healthy' : null,
          restart_policy: found.compose_project ? 'unless-stopped' : 'no',
          restart_retries: 0,
          hostname: found.id.slice(0, 12),
          user: null,
          working_directory: '/',
          platform: 'linux',
          mounts: found.compose_project
            ? [
                {
                  kind: 'volume',
                  name: `${found.compose_project}_data`,
                  source: `/var/lib/docker/volumes/${found.compose_project}_data/_data`,
                  destination: '/data',
                  read_write: true,
                },
              ]
            : [],
          networks: [
            {
              name: found.compose_project ? `${found.compose_project}_default` : 'bridge',
              ip_address: '172.18.0.2',
              ipv6_address: null,
              gateway: '172.18.0.1',
              mac_address: null,
              aliases: found.names,
            },
          ],
        })
      },
    ),
    http.get('*/api/v1/container-engines/:engine/disk-usage', async () => {
      await new Promise((resolve) => setTimeout(resolve, 600))
      return HttpResponse.json({
        observed_at: new Date().toISOString(),
        images: {
          total: images.length,
          active: images.filter((image) => image.containers > 0).length,
          size_bytes: 397 * 1024 ** 2,
          reclaimable_bytes: 88 * 1024 ** 2,
        },
        containers: {
          total: 4,
          active: 3,
          size_bytes: 12 * 1024 ** 2,
          reclaimable_bytes: 2 * 1024 ** 2,
        },
        volumes: { total: 1, active: 1, size_bytes: 640 * 1024 ** 2, reclaimable_bytes: 0 },
        build_cache: { total: 0, active: 0, size_bytes: 0, reclaimable_bytes: 0 },
      } satisfies EngineDiskUsageView)
    }),
    http.get('*/api/v1/container-engines/:engine/networks', () =>
      HttpResponse.json({
        observed_at: new Date().toISOString(),
        networks: [
          {
            id: 'n1',
            name: 'bridge',
            driver: 'bridge',
            scope: 'local',
            created: hoursAgo(24 * 90),
            internal: false,
            ipv6: false,
            subnets: [{ subnet: '172.17.0.0/16', gateway: '172.17.0.1' }],
            containers: 0,
            compose_project: null,
            labels: {},
          },
          {
            id: 'n3',
            name: 'shop_default',
            driver: 'bridge',
            scope: 'local',
            created: hoursAgo(24 * 30),
            internal: false,
            ipv6: false,
            subnets: [{ subnet: '172.18.0.0/16', gateway: '172.18.0.1' }],
            containers: 2,
            compose_project: 'shop',
            labels: { 'com.docker.compose.project': 'shop' },
          },
        ],
      } satisfies EngineNetworkListView),
    ),
    http.get('*/api/v1/container-engines/:engine/volumes', () =>
      HttpResponse.json({
        observed_at: new Date().toISOString(),
        volumes: [
          {
            name: 'shop_data',
            driver: 'local',
            mountpoint: '/var/lib/docker/volumes/shop_data/_data',
            created: hoursAgo(24 * 30),
            scope: 'local',
            containers: 1,
            compose_project: 'shop',
            labels: { 'com.docker.compose.project': 'shop' },
          },
        ],
      } satisfies EngineVolumeListView),
    ),
    http.get('*/api/v1/container-engines/:engine/images', ({ request }) => {
      const search = (new URL(request.url).searchParams.get('search') ?? '').toLowerCase()
      return HttpResponse.json({
        observed_at: new Date().toISOString(),
        images: images.filter(
          (image) =>
            !search ||
            image.id.includes(search) ||
            image.tags.some((tag) => tag.toLowerCase().includes(search)),
        ),
      } satisfies ImageListView)
    }),
    http.get<{ engine: string; image: string }>(
      '*/api/v1/container-engines/:engine/images/:image',
      ({ params }) => {
        const found = images.find(
          (image) => image.id === params.image || image.tags.includes(params.image),
        )
        if (!found) {
          return missing(params.image)
        }
        return HttpResponse.json({
          image: found,
          architecture: 'amd64',
          variant: null,
          os: 'linux',
          author: found.labels.maintainer ?? null,
          comment: null,
          user: null,
          working_directory: '/',
          exposed_ports: found.tags[0]?.startsWith('nginx') ? ['80/tcp'] : [],
          volumes: [],
          stop_signal: 'SIGQUIT',
          layers: 7,
        } satisfies ImageDetailView)
      },
    ),
    http.delete<{ engine: string; image: string }>(
      '*/api/v1/container-engines/:engine/images/:image',
      ({ params }) => {
        const index = images.findIndex((image) => image.id === params.image)
        const found = images[index]
        if (!found) {
          return missing(params.image)
        }
        if (found.containers > 0) {
          return HttpResponse.json(
            {
              type: 'about:blank',
              title: 'Conflict',
              status: 409,
              code: 'CONFLICT',
              detail: `unable to delete ${found.id} - image is being used by a container`,
            },
            { status: 409, headers: { 'content-type': 'application/problem+json' } },
          )
        }
        images.splice(index, 1)
        return HttpResponse.json({ id: found.id, untagged: found.tags, deleted: [found.id] })
      },
    ),
    http.get('*/api/v1/container-engines/:engine/stats', () =>
      HttpResponse.json({
        observed_at: new Date().toISOString(),
        stats: containers.filter((container) => container.state === 'running').map(usage),
      } satisfies ContainerStatsListView),
    ),
    http.get<{ engine: string; container: string }>(
      '*/api/v1/container-engines/:engine/containers/:container/stats',
      ({ params }) => {
        const found = containers.find((container) => container.id === params.container)
        if (!found) {
          return missing(params.container)
        }
        return found.state === 'running'
          ? HttpResponse.json(usage(found))
          : HttpResponse.json(
              {
                type: 'about:blank',
                title: 'Precondition failed',
                status: 412,
                code: 'PRECONDITION_FAILED',
                detail: `${found.names[0]} is not running`,
              },
              { status: 412, headers: { 'content-type': 'application/problem+json' } },
            )
      },
    ),
    http.get<{ engine: string; container: string }>(
      '*/api/v1/container-engines/:engine/containers/:container/logs',
      ({ params, request }) => {
        if (!containers.some((container) => container.id === params.container)) {
          return missing(params.container)
        }
        const count = Number(new URL(request.url).searchParams.get('lines')) || 200
        const now = Date.now()
        const lines = Array.from({ length: Math.min(count, 300) }, (_, index) =>
          printed(now - (300 - index) * PRINT_INTERVAL_MS, index),
        )
        return HttpResponse.json({
          observed_at: new Date(now).toISOString(),
          lines,
          truncated: false,
        } satisfies ContainerLogsView)
      },
    ),
    tail.addEventListener('connection', ({ client }) => {
      const id = new URL(client.url).pathname.split('/').at(-3)
      const found = containers.find((container) => container.id === id)
      if (found?.state !== 'running') {
        client.close(1000, 'the container stopped')
        return
      }
      let index = 0
      const timer = setInterval(() => {
        const message: ContainerLogTailMessage = {
          lines: [printed(Date.now(), (index += 1))],
          cursor: new Date().toISOString(),
        }
        client.send(JSON.stringify(message))
      }, PRINT_INTERVAL_MS)
      client.addEventListener('close', () => clearInterval(timer))
    }),
    http.post<{ engine: string; container: string; action: string }>(
      '*/api/v1/container-engines/:engine/containers/:container/:action',
      ({ params }) => {
        const found = containers.find((container) => container.id === params.container)
        if (!found) {
          return missing(params.container)
        }
        const running = params.action === 'start' || params.action === 'restart'
        Object.assign(found, {
          state: running ? 'running' : 'exited',
          status: running
            ? 'Up 1 second'
            : `Exited (${params.action === 'kill' ? 137 : 0}) 1 second ago`,
        })
        return HttpResponse.json({ id: found.id, name: found.names[0], container: found })
      },
    ),
    http.delete<{ engine: string; container: string }>(
      '*/api/v1/container-engines/:engine/containers/:container',
      ({ params, request }) => {
        const index = containers.findIndex((container) => container.id === params.container)
        const found = containers[index]
        if (!found) {
          return missing(params.container)
        }
        const force = new URL(request.url).searchParams.get('force') === 'true'
        if (found.state === 'running' && !force) {
          return HttpResponse.json(
            {
              type: 'about:blank',
              title: 'Conflict',
              status: 409,
              code: 'CONFLICT',
              detail: 'You cannot remove a running container',
            },
            { status: 409 },
          )
        }
        containers.splice(index, 1)
        return HttpResponse.json({ id: found.id, name: found.names[0], container: null })
      },
    ),
    http.get('*/api/v1/container-engines/:engine/containers', ({ params, request }) => {
      const url = new URL(request.url)
      const search = (url.searchParams.get('search') ?? '').toLowerCase()
      const states = (url.searchParams.get('state') ?? '').split(',').filter(Boolean)
      const listed = params.engine === 'docker' ? containers : []
      return HttpResponse.json({
        observed_at: new Date().toISOString(),
        containers: listed.filter(
          (container) =>
            (!search ||
              container.names.some((name) => name.toLowerCase().includes(search)) ||
              container.image.toLowerCase().includes(search)) &&
            (states.length === 0 || states.includes(container.state)),
        ),
      })
    }),
  ]
}
