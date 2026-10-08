import { describe, expect, it } from 'vitest'
import { bundleName, engineName, readinessTone, shortDigest } from '../presentation'

describe('system presentation', () => {
  it('reads every readiness state with its own tone', () => {
    expect(readinessTone('pass')).toBe('positive')
    expect(readinessTone('warn')).toBe('warning')
    expect(readinessTone('fail')).toBe('negative')
    expect(readinessTone('later')).toBe('neutral')
  })

  it('names the bundle after when it was put together', () => {
    expect(bundleName('2026-10-08T12:34:56Z')).toBe(
      'pingora-panel-diagnostics-20261008T123456Z.json',
    )
    expect(bundleName('2026-10-08T12:34:56.789Z')).toBe(
      'pingora-panel-diagnostics-20261008T123456Z.json',
    )
  })

  it('shortens digests and names engines', () => {
    expect(shortDigest(`sha256:${'ab'.repeat(32)}`)).toBe('sha256:abababababab')
    expect(shortDigest('local')).toBe('local')
    expect(engineName('podman')).toBe('Podman')
    expect(engineName('other')).toBe('other')
  })
})
