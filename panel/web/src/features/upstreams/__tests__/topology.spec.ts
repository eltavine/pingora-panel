import { describe, expect, it } from 'vitest'
import type { SiteView, UpstreamHealthReportResponse, UpstreamView } from '@/api/generated'
import { topology } from '../topology'

const SHOP = '0b9d6c52-2f47-4d0e-9a1b-6f3c2d1e0a01'
const BLOG = '1c8e7d63-3058-4e1f-8b2c-7a4d3e2f1b02'
const APP = '7e1f0c3a-5b2d-4c8e-9f60-1a2b3c4d5e6f'
const ROUTED = '8f2a1d4b-6c3e-4d9f-a071-2b3c4d5e6f70'
const IDLE = '9a3b2e5c-7d4f-4ea0-b182-3c4d5e6f7081'

function site(id: string, name: string, extra: Partial<SiteView>): SiteView {
  return {
    id,
    name,
    action: { type: 'static', root: name },
    enabled: true,
    domains: [{ host: `${name}.example` }],
    routes: [],
    etag: '"1"',
    https: false,
    kind: 'reverse_proxy',
    status: 'running',
    ...extra,
  } as SiteView
}

function pool(id: string, name: string, nodes: UpstreamView['nodes']): UpstreamView {
  return { id, name, nodes, used_by: [], etag: '"1"' } as unknown as UpstreamView
}

const route = (
  id: string,
  upstream: string,
  enabled = true,
): NonNullable<SiteView['routes']>[number] =>
  ({
    id,
    enabled,
    priority: 1,
    match: { kind: 'prefix', path: '/api' },
    action: { type: 'proxy', upstream_id: upstream },
    name: null,
  }) as NonNullable<SiteView['routes']>[number]

describe('upstream topology', () => {
  const upstreams = [
    pool(APP, 'app', [
      { id: 'a1', host: '10.0.0.11', port: 8080, enabled: true },
      { id: 'a2', host: '::1', port: 8080, enabled: true, backup: true },
    ] as UpstreamView['nodes']),
    pool(ROUTED, 'api', [
      { id: 'p1', host: '10.0.0.21', port: 9000, enabled: false },
    ] as UpstreamView['nodes']),
    pool(IDLE, 'idle', []),
  ]

  it('follows sites and their routes to pools and nodes', () => {
    const sites = [
      site(SHOP, 'shop', {
        action: { type: 'proxy', upstream_id: APP },
        routes: [route('r1', ROUTED), route('r2', ROUTED, false)],
      }),
      site(BLOG, 'blog', {}),
    ]
    const graph = topology(sites, upstreams)
    expect(graph.sites.map((item) => [item.name, item.pool])).toEqual([['shop', APP]])
    expect(graph.routes.map((item) => [item.id, item.pool])).toEqual([['r1', ROUTED]])
    expect(graph.edges).toEqual([
      [`site:${SHOP}`, `pool:${APP}`],
      [`site:${SHOP}`, 'route:r1'],
      ['route:r1', `pool:${ROUTED}`],
      [`pool:${APP}`, `node:${APP}/a1`],
      [`pool:${APP}`, `node:${APP}/a2`],
      [`pool:${ROUTED}`, `node:${ROUTED}/p1`],
    ])
    expect(graph.pools.map((item) => [item.name, item.used])).toEqual([
      ['app', true],
      ['api', true],
      ['idle', false],
    ])
    expect(graph.nodes.map((item) => [item.address, item.backup, item.state])).toEqual([
      ['10.0.0.11:8080', false, 'unchecked'],
      ['[::1]:8080', true, 'unchecked'],
      ['10.0.0.21:9000', false, 'disabledNode'],
    ])
  })

  it('leaves out disabled sites and those that reach no known pool', () => {
    const sites = [
      site(SHOP, 'shop', { enabled: false, action: { type: 'proxy', upstream_id: APP } }),
      site(BLOG, 'blog', { action: { type: 'proxy', upstream_id: 'gone' } }),
    ]
    const graph = topology(sites, upstreams)
    expect(graph.sites).toEqual([])
    expect(graph.pools.every((item) => !item.used)).toBe(true)
  })

  it('takes node health from the gateway', () => {
    const health = {
      upstreams: [
        {
          upstream_id: APP,
          checked: true,
          nodes: [
            { node_id: 'a1', healthy: true, drained: false, in_flight: 2, latency_us: 1830 },
            {
              node_id: 'a2',
              healthy: true,
              drained: false,
              ejected_until: '2026-10-08T10:00:30Z',
              in_flight: 0,
            },
          ],
        },
      ],
    } as unknown as UpstreamHealthReportResponse
    const graph = topology([], upstreams, health, Date.parse('2026-10-08T10:00:00Z'))
    expect(graph.nodes.map((item) => item.state)).toEqual(['healthy', 'ejected', 'disabledNode'])
    expect(graph.pools[0]).toMatchObject({ healthy: 1, total: 2 })
    expect(graph.nodes[0]!.health?.in_flight).toBe(2)
  })
})
