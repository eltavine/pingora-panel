import { http, HttpResponse, type AnyHandler } from 'msw'
import type { ContainerEngineView, ContainerView } from '@/api/generated'

const GIB = 1024 ** 3

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
