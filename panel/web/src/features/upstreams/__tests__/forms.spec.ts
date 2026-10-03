import { describe, expect, it } from 'vitest'
import type { EndpointHealthResponse, UpstreamNode, UpstreamView } from '@/api/generated'
import {
  algorithmOf,
  latency,
  nodeAddress,
  nodeState,
  parseNodes,
  upstreamForm,
  upstreamInput,
} from '../forms'

const node: UpstreamNode = { id: 'n1', host: '10.0.0.1', port: 8080, weight: 2 }
const health: EndpointHealthResponse = {
  node_id: 'n1',
  address: '10.0.0.1:8080',
  weight: 2,
  enabled: true,
  backup: false,
  healthy: true,
  drained: false,
  in_flight: 0,
  requests: 10,
  failures: 0,
}

describe('parseNodes', () => {
  it('reads addresses, weights and flags', () => {
    const { nodes, invalid } = parseNodes(
      '10.0.0.1:8080 weight=3\n[2001:db8::1]:443 tls backup\n# comment\n\napp.internal:80 w=2',
    )
    expect(invalid).toEqual([])
    expect(nodes).toEqual([
      { host: '10.0.0.1', port: 8080, weight: 3 },
      { host: '2001:db8::1', port: 443, weight: 1, tls: true, backup: true },
      { host: 'app.internal', port: 80, weight: 2 },
    ])
  })

  it('reports lines it cannot read', () => {
    expect(
      parseNodes('10.0.0.1\n10.0.0.2:70000\n10.0.0.3:80 fast\n2001:db8::1:80').invalid,
    ).toEqual([1, 2, 3, 4])
  })
})

describe('upstream forms', () => {
  it('maps the balancing choice to the policy shape', () => {
    const form = upstreamForm()
    form.name = 'api'
    form.nodes = '10.0.0.1:8080'
    expect(upstreamInput(form).balancing).toBe('round_robin')
    form.algorithm = 'consistent_hash'
    form.hashKey = ' header:x-user '
    expect(upstreamInput(form).balancing).toEqual({ consistent_hash: { key: 'header:x-user' } })
    expect(algorithmOf({ consistent_hash: { key: 'uri' } })).toBe('consistent_hash')
    expect(algorithmOf(undefined)).toBe('round_robin')
  })

  it('omits disabled health checks and keeps nodes when replacing', () => {
    const upstream = {
      id: 'u1',
      name: 'api',
      nodes: [node],
      health_check: null,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      etag: '"e"',
      used_by: [],
    } satisfies UpstreamView
    const form = upstreamForm(upstream)
    expect(form.healthEnabled).toBe(false)
    form.passiveEnabled = true
    form.connectTimeout = ''
    const input = upstreamInput(form, upstream)
    expect(input.nodes).toEqual([node])
    expect(input.health_check).toBeNull()
    expect(input.passive_health).toEqual({ failure_threshold: 5, ejection_ms: 30000 })
    expect(input.connection?.connect_timeout_ms).toBeNull()
  })

  it('parses expected statuses and ignores junk', () => {
    const form = upstreamForm()
    form.healthEnabled = true
    form.expectedStatuses = '200, 204, x, '
    expect(upstreamInput(form).health_check?.expected_statuses).toEqual([200, 204])
  })
})

describe('node state', () => {
  it('reports the most specific state first', () => {
    const now = Date.parse('2026-01-01T00:00:00Z')
    expect(nodeState({ ...node, enabled: false }, health, now)).toBe('disabledNode')
    expect(nodeState(node, undefined, now)).toBe('unchecked')
    expect(nodeState(node, { ...health, drained: true }, now)).toBe('drained')
    expect(nodeState(node, { ...health, ejected_until: '2026-01-01T00:00:05Z' }, now)).toBe(
      'ejected',
    )
    expect(nodeState(node, { ...health, ejected_until: '2025-12-31T00:00:00Z' }, now)).toBe(
      'healthy',
    )
    expect(nodeState(node, { ...health, healthy: false }, now)).toBe('unhealthy')
  })

  it('formats addresses and latency', () => {
    expect(nodeAddress(node)).toBe('10.0.0.1:8080')
    expect(nodeAddress({ host: '::1', port: 80 })).toBe('[::1]:80')
    expect(latency(null)).toBe('—')
    expect(latency(1234)).toBe('1.23 ms')
    expect(latency(56_789)).toBe('56.8 ms')
  })
})
