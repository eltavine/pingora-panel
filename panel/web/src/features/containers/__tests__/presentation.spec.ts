import { describe, expect, it } from 'vitest'
import type { ContainerEngineView, ProblemDetails } from '@/api/generated'
import {
  chosenEngine,
  conditionTone,
  engineCondition,
  portLabel,
  sortedLabels,
  stoppable,
  withoutAgent,
} from '../presentation'
import { engineName, stateTone } from '@/lib/containers'

function engine(id: string, enabled: boolean, reachable: boolean): ContainerEngineView {
  return { id, socket: `/run/${id}.sock`, enabled, reachable }
}

describe('container engines', () => {
  it('tell an engine left alone from one that does not answer', () => {
    expect(engineCondition(engine('docker', false, true))).toBe('disabled')
    expect(engineCondition(engine('docker', true, false))).toBe('unreachable')
    expect(engineCondition(engine('docker', true, true))).toBe('reachable')
    expect(conditionTone('unreachable')).toBe('negative')
    expect(conditionTone('disabled')).toBe('neutral')
  })

  it('show the engine asked for, else the first that answers', () => {
    const engines = [engine('podman', false, false), engine('docker', true, true)]
    expect(chosenEngine(engines, 'podman')).toBe('podman')
    expect(chosenEngine(engines, '')).toBe('docker')
    expect(chosenEngine(engines, 'containerd')).toBe('docker')
    expect(chosenEngine([engine('podman', true, false)], '')).toBe('podman')
    expect(chosenEngine([], 'docker')).toBeNull()
  })

  it('go by their product names, and unknown ones by their identifiers', () => {
    expect(engineName('docker')).toBe('Docker')
    expect(engineName('podman')).toBe('Podman')
    expect(engineName('nerdctl')).toBe('nerdctl')
  })

  it('explain a missing agent rather than report it as a failure', () => {
    const problem = (code: string) =>
      ({ kind: 'problem', problem: { title: code, status: 422, code } as ProblemDetails }) as const
    expect(withoutAgent(problem('UNSUPPORTED_CAPABILITY'))).toBe(true)
    expect(withoutAgent(problem('UNAVAILABLE'))).toBe(false)
    expect(withoutAgent({ kind: 'unreachable' })).toBe(false)
  })
})

describe('containers', () => {
  it('write ports as docker ps does', () => {
    expect(
      portLabel({ private_port: 80, public_port: 8081, host_ip: '0.0.0.0', protocol: 'tcp' }),
    ).toBe('0.0.0.0:8081->80/tcp')
    expect(portLabel({ private_port: 53, public_port: 5353, host_ip: '::', protocol: 'udp' })).toBe(
      '[::]:5353->53/udp',
    )
    expect(portLabel({ private_port: 443, public_port: null, host_ip: '', protocol: 'tcp' })).toBe(
      '443/tcp',
    )
  })

  it('give running containers a positive tone and dead ones a negative one', () => {
    expect(stateTone('running')).toBe('positive')
    expect(stateTone('restarting')).toBe('pending')
    expect(stateTone('paused')).toBe('warning')
    expect(stateTone('dead')).toBe('negative')
    expect(stateTone('exited')).toBe('neutral')
    expect(stateTone('unknown')).toBe('neutral')
  })

  it('list labels by name', () => {
    expect(
      sortedLabels({
        'org.opencontainers.image.version': '1.27',
        'com.docker.compose.project': 'shop',
      }),
    ).toEqual([
      { name: 'com.docker.compose.project', value: 'shop' },
      { name: 'org.opencontainers.image.version', value: '1.27' },
    ])
  })

  it('offer stopping only what has something running', () => {
    expect(stoppable('running')).toBe(true)
    expect(stoppable('paused')).toBe(true)
    expect(stoppable('restarting')).toBe(true)
    expect(stoppable('exited')).toBe(false)
    expect(stoppable('created')).toBe(false)
  })
})
