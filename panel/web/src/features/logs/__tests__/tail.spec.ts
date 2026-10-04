import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { LogTailMessage } from '@/api/generated'
import { TAIL_LIMIT, useLogTail, type TailSocket } from '../tail'

class FakeSocket implements TailSocket {
  closed = false
  onopen: TailSocket['onopen'] = null
  onmessage: TailSocket['onmessage'] = null
  onclose: TailSocket['onclose'] = null

  constructor(readonly after?: string) {}

  close() {
    this.closed = true
  }

  open() {
    this.onopen?.call(this as unknown as WebSocket, new Event('open'))
  }

  send(message: LogTailMessage) {
    this.onmessage?.call(
      this as unknown as WebSocket,
      new MessageEvent('message', { data: JSON.stringify(message) }),
    )
  }

  drop() {
    this.onclose?.call(this as unknown as WebSocket, new CloseEvent('close', { code: 1006 }))
  }
}

function lines(...texts: string[]) {
  return texts.map((line, index) => ({
    time: `2026-10-04T10:00:0${index}Z`,
    kind: 'access' as const,
    line,
    fields: {},
  }))
}

describe('following logs', () => {
  let sockets: FakeSocket[]
  const connect = (after?: string) => {
    const socket = new FakeSocket(after)
    sockets.push(socket)
    return socket
  }

  beforeEach(() => {
    sockets = []
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('keeps the newest records first, up to the limit', () => {
    const tail = useLogTail(connect)
    tail.start('2026-10-04T09:00:00Z')
    expect(sockets[0]!.after).toBe('2026-10-04T09:00:00Z')
    sockets[0]!.open()
    expect(tail.state.value).toBe('live')

    sockets[0]!.send({ records: lines('one', 'two'), cursor: '2026-10-04T10:00:01Z' })
    sockets[0]!.send({ records: lines('three'), cursor: '2026-10-04T10:00:02Z' })
    expect(tail.records.value.map((record) => record.line)).toEqual(['three', 'two', 'one'])

    const many = Array.from({ length: TAIL_LIMIT }, (_, index) => `line ${index}`)
    sockets[0]!.send({ records: lines(...many), cursor: '2026-10-04T10:00:03Z' })
    expect(tail.records.value).toHaveLength(TAIL_LIMIT)
    expect(tail.records.value[0]!.line).toBe(`line ${TAIL_LIMIT - 1}`)
  })

  it('resumes from the cursor after falling behind', () => {
    const tail = useLogTail(connect)
    tail.start()
    sockets[0]!.open()
    sockets[0]!.send({
      records: [],
      cursor: '2026-10-04T10:00:05Z',
      error: { code: 'RESOURCE_EXHAUSTED', message: 'the tail fell behind' },
    })
    expect(sockets[0]!.closed).toBe(true)
    expect(tail.state.value).toBe('connecting')
    vi.advanceTimersByTime(250)
    expect(sockets[1]!.after).toBe('2026-10-04T10:00:05Z')
  })

  it('stops on other errors and gives up on a connection that keeps dropping', () => {
    const tail = useLogTail(connect)
    tail.start()
    sockets[0]!.send({
      records: [],
      error: { code: 'INVALID_ARGUMENT', message: 'status is not a status code' },
    })
    expect(tail.state.value).toBe('failed')
    expect(tail.failure.value?.code).toBe('INVALID_ARGUMENT')

    tail.start()
    for (let attempt = 0; attempt < 5; attempt += 1) {
      sockets.at(-1)!.drop()
      vi.runOnlyPendingTimers()
    }
    expect(tail.state.value).toBe('connecting')
    sockets.at(-1)!.drop()
    expect(tail.state.value).toBe('failed')
    expect(tail.failure.value).toEqual({})
  })

  it('closes its socket when stopped and ignores it afterwards', () => {
    const tail = useLogTail(connect)
    tail.start()
    sockets[0]!.open()
    tail.stop()
    expect(sockets[0]!.closed).toBe(true)
    expect(tail.state.value).toBe('idle')
    sockets[0]!.send({ records: lines('late') })
    sockets[0]!.drop()
    expect(tail.records.value).toEqual([])
    expect(sockets).toHaveLength(1)
  })
})
