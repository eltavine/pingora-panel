import { describe, expect, it } from 'vitest'
import type { DirectoryUsageView, HostSummaryView, PortListenerView } from '@/api/generated'
import {
  agentTone,
  capabilityTone,
  directoryNotes,
  gatewayHealthKey,
  gatewayTone,
  holderTone,
  levelTone,
  memoryUsed,
  portHolder,
  uptimeParts,
} from '../presentation'

describe('host figures', () => {
  it('read full filesystems as warnings and critical ones as negative', () => {
    expect(levelTone('critical')).toBe('negative')
    expect(levelTone('warning')).toBe('warning')
    expect(levelTone('ok')).toBe('positive')
  })

  it('give uptime in its two largest units', () => {
    expect(uptimeParts(90_000)).toEqual([
      { unit: 'days', n: 1 },
      { unit: 'hours', n: 1 },
    ])
    expect(uptimeParts(2 * 86_400)).toEqual([{ unit: 'days', n: 2 }])
    expect(uptimeParts(3_660)).toEqual([
      { unit: 'hours', n: 1 },
      { unit: 'minutes', n: 1 },
    ])
    expect(uptimeParts(59)).toEqual([{ unit: 'minutes', n: 0 }])
  })

  it('read memory in use as a share', () => {
    const host = { memory_total_bytes: 8, memory_available_bytes: 2 } as HostSummaryView
    expect(memoryUsed(host)).toBe(0.75)
    expect(memoryUsed({ ...host, memory_total_bytes: 0 })).toBe(0)
  })
})

describe('host agent', () => {
  it('reads a missing agent as neutral and a silent one as negative', () => {
    expect(agentTone('connected')).toBe('positive')
    expect(agentTone('not_configured')).toBe('neutral')
    expect(agentTone('unreachable')).toBe('negative')
  })

  it('warns about missing privileges and leaves disabled capabilities neutral', () => {
    expect(capabilityTone('available')).toBe('positive')
    expect(capabilityTone('denied')).toBe('warning')
    expect(capabilityTone('unreachable')).toBe('negative')
    expect(capabilityTone('not_enabled')).toBe('neutral')
    expect(capabilityTone('unsupported')).toBe('neutral')
  })

  it("reads the gateway container's state with its health check", () => {
    expect(gatewayTone('running', null)).toBe('positive')
    expect(gatewayTone('running', 'healthy')).toBe('positive')
    expect(gatewayTone('running', 'unhealthy')).toBe('negative')
    expect(gatewayTone('running', 'starting')).toBe('pending')
    expect(gatewayTone('restarting', null)).toBe('pending')
    expect(gatewayTone('exited', 'unhealthy')).toBe('neutral')
    expect(gatewayHealthKey('unhealthy')).toBe('unhealthy')
    expect(gatewayHealthKey(null)).toBeNull()
    expect(gatewayHealthKey('none')).toBeNull()
  })

  it('tells the gateway holding a port from another process holding it', () => {
    const process = (name: string) => ({ pid: 1, name, executable: null, uid: 0 })
    const listener = (names: string[]): PortListenerView => ({
      address: '0.0.0.0',
      port: 443,
      uid: 0,
      processes: names.map(process),
    })
    expect(portHolder(listener(['gatewayd']))).toBe('gateway')
    expect(portHolder(listener(['gatewayd', 'nginx']))).toBe('other')
    expect(portHolder(listener([]))).toBe('unknown')
    expect(holderTone('gateway')).toBe('positive')
    expect(holderTone('other')).toBe('warning')
    expect(holderTone('unknown')).toBe('neutral')
  })

  it('qualifies directories that are missing, partial or partly unreadable', () => {
    const directory: DirectoryUsageView = {
      kind: 'logs',
      path: '/var/log/pingora-panel',
      present: true,
      bytes: 10,
      files: 1,
      unreadable: 0,
      truncated: false,
    }
    expect(directoryNotes(directory)).toEqual([])
    expect(directoryNotes({ ...directory, present: false, truncated: true })).toEqual([
      { kind: 'missing' },
    ])
    expect(directoryNotes({ ...directory, truncated: true, unreadable: 3 })).toEqual([
      { kind: 'partial' },
      { kind: 'unreadable', n: 3 },
    ])
  })
})
