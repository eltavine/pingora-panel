import { describe, expect, it } from 'vitest'
import type {
  ComposeLogLineView,
  ContainerEngineView,
  ContainerLogLineView,
  ContainerStatsView,
  ImageLayerStateName,
  ImageLayerView,
  ProblemDetails,
} from '@/api/generated'
import {
  chosenEngine,
  conditionTone,
  engineCondition,
  finished,
  layersDone,
  logFile,
  logTailUrl,
  matchesLine,
  matchesProjectLine,
  memoryShare,
  plainText,
  portLabel,
  projectCondition,
  projectLogFile,
  projectTone,
  pullShare,
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

describe('container logs', () => {
  const escape = String.fromCharCode(27)
  const line = (text: string, stream: 'stdout' | 'stderr' = 'stdout'): ContainerLogLineView => ({
    time: '2027-01-15T08:00:01.000000001Z',
    stream,
    text,
  })

  it('reads lines without their terminal control sequences', () => {
    expect(plainText(`${escape}[1;32mGET${escape}[0m / 200`)).toBe('GET / 200')
    expect(plainText(`${escape}]0;title${String.fromCharCode(7)}ready`)).toBe('ready')
    expect(plainText(`${escape}]8;;https://example.com${escape}\\link`)).toBe('link')
    expect(plainText('plain [brackets] stay')).toBe('plain [brackets] stay')
  })

  it('filters lines by stream and by text, ignoring case and colours', () => {
    const coloured = line(`${escape}[31mUpstream${escape}[0m timed out`, 'stderr')
    expect(matchesLine(coloured, 'all', 'upstream TIMED')).toBe(true)
    expect(matchesLine(coloured, 'stdout', '')).toBe(false)
    expect(matchesLine(coloured, 'stderr', '  ')).toBe(true)
    expect(matchesLine(line('GET /'), 'all', 'post')).toBe(false)
  })

  it('saves lines as printed, each after its time', () => {
    expect(logFile([line('one'), line('two', 'stderr')])).toBe(
      '2027-01-15T08:00:01.000000001Z one\n2027-01-15T08:00:01.000000001Z two\n',
    )
  })

  it('follows a container over a WebSocket where the API answers', () => {
    const page = 'https://panel.example/containers'
    expect(logTailUrl('docker', 'shop-web-1', { lines: '0' }, { baseUrl: '', page })).toBe(
      'wss://panel.example/api/v1/container-engines/docker/containers/shop-web-1/logs/tail?lines=0',
    )
    expect(
      logTailUrl(
        'podman',
        'b2',
        { after: '2027-01-15T08:00:01Z' },
        { baseUrl: 'http://127.0.0.1:8080', page },
      ),
    ).toBe(
      'ws://127.0.0.1:8080/api/v1/container-engines/podman/containers/b2/logs/tail?after=2027-01-15T08%3A00%3A01Z',
    )
  })
})

describe('container usage', () => {
  it('shares memory out of its limit, and none without one', () => {
    const stats = { memory_bytes: 256, memory_limit_bytes: 1_024 } as ContainerStatsView
    expect(memoryShare(stats)).toBe(0.25)
    expect(memoryShare({ ...stats, memory_limit_bytes: 0 })).toBeUndefined()
  })
})

describe('compose projects', () => {
  it('run fully, in part or not at all, each with its own glyph', () => {
    expect(projectCondition({ running: 2, containers: 2 })).toBe('running')
    expect(projectCondition({ running: 1, containers: 2 })).toBe('partial')
    expect(projectCondition({ running: 0, containers: 2 })).toBe('stopped')
    expect(projectTone('running')).toBe('positive')
    expect(projectTone('partial')).toBe('warning')
    expect(projectTone('stopped')).toBe('neutral')
  })

  const lines: ComposeLogLineView[] = [
    {
      service: 'web',
      container: 'shop-web-1',
      line: { time: '2027-01-15T08:00:00Z', stream: 'stdout', text: 'GET /cart' },
    },
    {
      service: 'db',
      container: 'shop-db-1',
      line: { time: '2027-01-15T08:00:01Z', stream: 'stderr', text: 'checkpoint' },
    },
  ]

  it('filter their lines by service as well as by output and text', () => {
    expect(lines.filter((line) => matchesProjectLine(line, '', 'all', ''))).toHaveLength(2)
    expect(lines.filter((line) => matchesProjectLine(line, 'db', 'all', ''))).toEqual([lines[1]])
    expect(lines.filter((line) => matchesProjectLine(line, 'web', 'stderr', ''))).toEqual([])
    expect(lines.filter((line) => matchesProjectLine(line, '', 'all', 'CART'))).toEqual([lines[0]])
  })

  it('save their lines after each container as docker compose logs prints them', () => {
    expect(projectLogFile(lines)).toBe(
      '2027-01-15T08:00:00Z shop-web-1 | GET /cart\n2027-01-15T08:00:01Z shop-db-1 | checkpoint\n',
    )
  })
})

describe('image pulls', () => {
  const layer = (state: ImageLayerStateName, current: number, total: number): ImageLayerView => ({
    id: `${state}-${current}`,
    state,
    current_bytes: current,
    total_bytes: total,
  })

  it('count downloading as the first half of a layer and extracting as the second', () => {
    expect(pullShare([])).toBeUndefined()
    expect(pullShare([layer('exists', 0, 4096)])).toBeUndefined()
    expect(pullShare([layer('downloading', 1024, 4096)])).toBe(0.125)
    expect(pullShare([layer('extracting', 2048, 4096)])).toBe(0.75)
    expect(pullShare([layer('complete', 0, 4096), layer('waiting', 0, 4096)])).toBe(0.5)
    expect(pullShare([layer('exists', 0, 4096), layer('downloaded', 0, 4096)])).toBe(0.5)
    expect(pullShare([layer('downloading', 9000, 4096)])).toBe(0.5)
  })

  it('are complete once their image is pulled, unless the engine had them', () => {
    expect(finished(layer('downloading', 1024, 4096))).toMatchObject({
      state: 'complete',
      current_bytes: 4096,
    })
    expect(finished(layer('exists', 0, 4096)).state).toBe('exists')
  })

  it('count the layers there is nothing more to do for', () => {
    expect(
      layersDone([layer('exists', 0, 0), layer('complete', 0, 1), layer('extracting', 0, 1)]),
    ).toBe(2)
  })
})
