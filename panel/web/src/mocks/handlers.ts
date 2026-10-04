import { http, HttpResponse, type AnyHandler } from 'msw'
import type {
  CurrentSession,
  DataPlaneResponse,
  DraftResponse,
  GatewayStatusResponse,
  HostSummaryView,
  SiteList,
  SiteSummary,
  SiteView,
  TrafficSeriesResponse,
  TrafficSummaryResponse,
  UpstreamHealthReportResponse,
  UpstreamView,
} from '@/api/generated'
import { alertHandlers } from './alerts'
import { hostAgentHandlers } from './host'
import { logHandlers } from './logs'
import type { Sampler } from './openapi'

/** Every permission of the catalog, so every page of the console shows. */
const PERMISSIONS = [
  'gateway.read',
  'gateway.operate',
  'gateway.publish',
  'config.read',
  'config.write',
  'config.apply',
  'approval.manage',
  'approval.decide',
  'approval.bypass',
  'certificate.read',
  'certificate.manage',
  'audit.read',
  'logs.read',
  'logs.delete',
  'alerts.read',
  'alerts.manage',
  'host.read',
  'host.manage',
  'platform.read',
  'identity.read',
  'identity.manage',
]

const started = new Date(Date.now() - 3 * 86_400_000).toISOString()
const activated = new Date(Date.now() - 2 * 3_600_000).toISOString()

function session(sampler: Sampler): CurrentSession {
  const sampled = sampler.schema<CurrentSession>('CurrentSession')
  return {
    ...sampled,
    account: { ...sampled.account, username: 'demo', display_name: 'Demo operator' },
    permissions: PERMISSIONS,
    limited: [],
    credential: 'cookie',
    csrf_token: 'demo-csrf-token',
  }
}

function sites(sampler: Sampler): SiteView[] {
  const sampled = sampler.schema<SiteView>('SiteView')
  return [
    { id: 'shop', name: 'Shop', kind: 'reverse_proxy', status: 'running', https: true },
    { id: 'blog', name: 'Blog', kind: 'static', status: 'running', https: true },
    { id: 'docs', name: 'Docs', kind: 'reverse_proxy', status: 'running', https: false },
    { id: 'legacy', name: 'Legacy', kind: 'redirect', status: 'stopped', https: false },
  ].map((site) => ({ ...sampled, ...site, etag: `"${site.id}-1"` }) as SiteView)
}

function upstreams(sampler: Sampler): UpstreamView[] {
  const sampled = sampler.schema<UpstreamView>('UpstreamView')
  return ['shop-app', 'docs-app'].map(
    (id) => ({ ...sampled, id, name: id, used_by: ['shop'], etag: `"${id}-1"` }) as UpstreamView,
  )
}

/** A day of traffic that rises and falls, with a few server errors. */
function series(windowSeconds: number): TrafficSeriesResponse {
  const points = 60
  const now = Date.now()
  return {
    points: Array.from({ length: points }, (_, index) => {
      const at = now - ((points - 1 - index) * windowSeconds * 1_000) / points
      const wave = Math.sin((index / points) * Math.PI * 2) * 4
      const rate = 12 + wave + Math.random() * 2
      return {
        at: new Date(at).toISOString(),
        requests_per_second: Number(rate.toFixed(2)),
        server_errors_per_second:
          index % 17 === 0 ? 0.4 : Number((Math.random() * 0.05).toFixed(3)),
        p95: Number((0.08 + Math.random() * 0.05 + (index % 17 === 0 ? 0.3 : 0)).toFixed(3)),
      }
    }),
  }
}

function summary(windowSeconds: number): TrafficSummaryResponse {
  const requests = 12.4 * windowSeconds
  return {
    observed_at: new Date().toISOString(),
    window_seconds: windowSeconds,
    requests,
    requests_per_second: 12.4,
    statuses: {
      informational: 0,
      success: requests * 0.93,
      redirection: requests * 0.04,
      client_error: requests * 0.025,
      server_error: requests * 0.005,
    },
    latency: { p50: 0.018, p90: 0.072, p95: 0.11, p99: 0.42 },
    bytes_received: requests * 820,
    bytes_sent: requests * 24_576,
    open_connections: 38,
    tls_handshakes: requests * 0.12,
    upstreams: [
      {
        upstream: 'shop-app',
        requests: requests * 0.7,
        error_ratio: 0.004,
        latency: { p50: 0.012, p90: 0.05, p95: 0.08, p99: 0.3 },
      },
      {
        upstream: 'docs-app',
        requests: requests * 0.2,
        error_ratio: 0,
        latency: { p50: 0.006, p90: 0.02, p95: 0.03, p99: 0.07 },
      },
    ],
    routes: [
      { site: 'shop', route: 'checkout', requests: requests * 0.38 },
      { site: 'shop', route: 'catalog', requests: requests * 0.24 },
      { site: 'docs', route: 'pages', requests: requests * 0.2 },
      { site: 'blog', route: 'posts', requests: requests * 0.1 },
    ],
    upstream_failures: [
      {
        upstream: 'docs-app',
        address: '10.0.1.11',
        port: 8080,
        error_type: 'connect_timeout',
        failures: Math.round(requests * 0.001),
      },
    ],
    domains: [
      { site: 'shop', domain: 'shop.example', requests: requests * 0.52 },
      { site: 'docs', domain: 'docs.example', requests: requests * 0.2 },
      { site: 'shop', domain: '*.shop.example', requests: requests * 0.1 },
      { site: 'blog', domain: 'blog.example', requests: requests * 0.1 },
    ],
    revision: 42,
    activated_at: activated,
  }
}

function windowOf(request: Request): number {
  return Number(new URL(request.url).searchParams.get('window')) || 3_600
}

/**
 * Hand-written answers for the pages people look at first, then an answer
 * shaped by the contract for every other operation.
 */
export function handlers(sampler: Sampler): AnyHandler[] {
  const status: GatewayStatusResponse = {
    ready: true,
    message: null,
    active_revision_id: 42,
    active_hash: 'sha256:4f1c9a0d5b7e2f8c',
    prepared_count: 0,
    adapter_version: '0.2.0',
    schema_version: 'v1',
  }
  const dataPlane: DataPlaneResponse = {
    active_revision_id: 42,
    active_hash: 'sha256:4f1c9a0d5b7e2f8c',
    adapter_version: '0.2.0',
    engine_version: '0.9.0',
    gateway_version: '0.1.0',
    generation: 7,
    generation_started_at: activated,
    started_at: started,
    observed_at: new Date().toISOString(),
    uptime_seconds: 3 * 86_400,
    worker_count: 4,
    listeners: [
      { id: 'http', address: '0.0.0.0:80', tls: false, http1: true, http2: false },
      { id: 'https', address: '0.0.0.0:443', tls: true, http1: true, http2: true },
    ],
  }
  const draft: DraftResponse = {
    version: 42,
    applied_version: 42,
    pending: false,
    updated_at: activated,
    applied_at: activated,
  }
  const siteSummary: SiteSummary = {
    total: 4,
    running: 3,
    stopped: 1,
    abnormal: 0,
    deleted: 0,
    https: 2,
    maintenance: 0,
    redirect: 1,
    reverse_proxy: 2,
    static: 1,
  }
  const health: UpstreamHealthReportResponse = {
    active_revision_id: 42,
    observed_at: new Date().toISOString(),
    upstreams: ['shop-app', 'docs-app'].map((upstream_id, index) => ({
      upstream_id,
      checked: true,
      nodes: [0, 1].map((node) => ({
        node_id: `node-${node}`,
        address: `10.0.${index}.${10 + node}:8080`,
        healthy: !(index === 1 && node === 1),
        enabled: true,
        backup: false,
        drained: false,
        failures: index === 1 && node === 1 ? 3 : 0,
        in_flight: 2 + node,
        requests: 18_000 - node * 4_000,
        weight: 1,
        latency_us: 9_000 + node * 2_500,
      })),
    })),
  }
  const host: HostSummaryView = {
    observed_at: new Date().toISOString(),
    reporting: true,
    hostname: 'edge-1',
    operating_system: 'Ubuntu 24.04.2 LTS',
    kernel_release: '6.8.0-41-generic',
    architecture: 'x86_64',
    host_time: new Date().toISOString(),
    time_zone: 'UTC',
    uptime_seconds: 19 * 86_400 + 4 * 3_600,
    cpu_count: 8,
    cpu_usage: 0.27,
    load1: 1.42,
    load5: 1.18,
    load15: 0.96,
    memory_total_bytes: 16 * 1024 ** 3,
    memory_available_bytes: 9.4 * 1024 ** 3,
    filesystems: [
      {
        mountpoint: '/var',
        device: '/dev/nvme1n1p1',
        fstype: 'xfs',
        size_bytes: 200 * 1024 ** 3,
        available_bytes: 22 * 1024 ** 3,
        used_ratio: 0.89,
        level: 'warning',
      },
      {
        mountpoint: '/',
        device: '/dev/nvme0n1p2',
        fstype: 'ext4',
        size_bytes: 100 * 1024 ** 3,
        available_bytes: 61 * 1024 ** 3,
        used_ratio: 0.39,
        level: 'ok',
      },
    ],
    network_devices: [
      { device: 'eth0', receive_bytes_per_second: 2.4e6, transmit_bytes_per_second: 9.1e6 },
    ],
  }
  const siteList = sites(sampler)
  return [
    http.get('*/api/v1/session', () => HttpResponse.json(session(sampler))),
    http.get('*/api/v1/gateway/status', () => HttpResponse.json(status)),
    http.get('*/api/v1/gateway/data-plane', () => HttpResponse.json(dataPlane)),
    http.get('*/api/v1/config/draft', () => HttpResponse.json(draft)),
    http.get('*/api/v1/sites/summary', () => HttpResponse.json(siteSummary)),
    http.get('*/api/v1/sites', () =>
      HttpResponse.json({
        items: siteList,
        total: siteList.length,
        next_cursor: null,
      } satisfies SiteList),
    ),
    http.get('*/api/v1/upstreams', () => HttpResponse.json(upstreams(sampler))),
    http.get('*/api/v1/upstreams/health', () => HttpResponse.json(health)),
    http.get('*/api/v1/traffic', ({ request }) => HttpResponse.json(summary(windowOf(request)))),
    http.get('*/api/v1/traffic/series', ({ request }) =>
      HttpResponse.json(series(windowOf(request))),
    ),
    http.get('*/api/v1/host', () => HttpResponse.json(host)),
    ...hostAgentHandlers(),
    ...logHandlers(),
    ...alertHandlers(),
    http.all('*/api/*', ({ request }) => {
      const answer = sampler.respond(request.method, new URL(request.url).pathname)
      if (!answer) {
        return HttpResponse.json(
          { type: 'about:blank', title: 'Not Found', status: 404 },
          { status: 404, headers: { 'content-type': 'application/problem+json' } },
        )
      }
      return answer.body === undefined
        ? new HttpResponse(null, { status: answer.status })
        : HttpResponse.json(answer.body as never, { status: answer.status })
    }),
  ]
}
