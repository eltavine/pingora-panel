import { getCurrentScope, onScopeDispose, ref, shallowRef } from 'vue'
import type { LogRecordItem, LogTailMessage } from '@/api/generated'

export type TailState = 'idle' | 'connecting' | 'live' | 'failed'

/** How many followed records are kept; older ones drop off. */
export const TAIL_LIMIT = 1_000
/** The pause before following again after falling behind. */
const RESUME_DELAY_MS = 250
/** Pauses before reconnecting after the connection drops, in turn. */
const RETRY_DELAYS_MS = [1_000, 2_000, 5_000, 10_000, 30_000]

/** The part of a WebSocket the tail uses, so tests can stand in for one. */
export type TailSocket = Pick<WebSocket, 'close' | 'onopen' | 'onmessage' | 'onclose'>

/** Why following stopped: the API's error, or the connection failing. */
export interface TailFailure {
  code?: string
  message?: string
}

/**
 * Follows records over a WebSocket (ADR 0026), newest first and at most
 * `TAIL_LIMIT` of them. It resumes from the last record after falling
 * behind or losing the connection, and gives up after the connection
 * fails `RETRY_DELAYS_MS.length` times in a row.
 */
export function useLogTail(connect: (after?: string) => TailSocket) {
  const state = ref<TailState>('idle')
  const records = shallowRef<LogRecordItem[]>([])
  const failure = ref<TailFailure>()
  let socket: TailSocket | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let cursor: string | undefined
  let failures = 0

  function resumeAfter(delay: number) {
    state.value = 'connecting'
    timer = setTimeout(open, delay)
  }

  function fail(reason: TailFailure) {
    state.value = 'failed'
    failure.value = reason
  }

  function open() {
    state.value = 'connecting'
    const current = connect(cursor)
    socket = current
    current.onopen = () => {
      if (socket === current) {
        state.value = 'live'
        failures = 0
      }
    }
    current.onmessage = (event: MessageEvent) => {
      if (socket !== current) {
        return
      }
      const message = JSON.parse(String(event.data)) as LogTailMessage
      if (message.records.length > 0) {
        const newest = [...message.records].reverse()
        records.value = [...newest, ...records.value].slice(0, TAIL_LIMIT)
      }
      cursor = message.cursor ?? cursor
      if (message.error) {
        socket = undefined
        current.close()
        if (message.error.code === 'RESOURCE_EXHAUSTED') {
          resumeAfter(RESUME_DELAY_MS)
        } else {
          fail(message.error)
        }
      }
    }
    current.onclose = () => {
      if (socket !== current) {
        return
      }
      socket = undefined
      const delay = RETRY_DELAYS_MS[failures]
      if (delay === undefined) {
        fail({})
        return
      }
      failures += 1
      resumeAfter(delay)
    }
  }

  /** Follows from `after`, or from now, dropping what was followed before. */
  function start(after?: string) {
    stop()
    records.value = []
    failure.value = undefined
    cursor = after
    failures = 0
    open()
  }

  function stop() {
    clearTimeout(timer)
    timer = undefined
    const current = socket
    socket = undefined
    current?.close()
    state.value = 'idle'
  }

  if (getCurrentScope()) {
    onScopeDispose(stop)
  }
  return { state, records, failure, start, stop }
}
