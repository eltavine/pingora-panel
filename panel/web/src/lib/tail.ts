import { getCurrentScope, onScopeDispose, ref, shallowRef } from 'vue'

export type TailState = 'idle' | 'connecting' | 'live' | 'ended' | 'failed'

/** The part of a WebSocket a tail uses, so tests can stand in for one. */
export type TailSocket = Pick<WebSocket, 'close' | 'onopen' | 'onmessage' | 'onclose'>

/** Why following stopped: the API's error, or the connection failing. */
export interface TailFailure {
  code?: string
  message?: string
}

/** What every message of a tail carries besides what it follows. */
export interface TailMessage {
  /** Where to resume after this message. */
  cursor?: string | null
  /** Why the tail ended; the last message carries it. */
  error?: TailFailure | null
}

export interface TailOptions<Item, Message extends TailMessage> {
  /** What a message carries, oldest first. */
  items: (message: Message) => readonly Item[]
  /** How many are kept; the oldest drop off. */
  limit: number
  /** Keeps the newest first rather than last. */
  newestFirst?: boolean
}

/** The pause before following again after falling behind. */
const RESUME_DELAY_MS = 250
/** Pauses before reconnecting after the connection drops, in turn. */
const RETRY_DELAYS_MS = [1_000, 2_000, 5_000, 10_000, 30_000]
/** How the API closes a tail that has nothing more to send. */
const NORMAL_CLOSURE = 1000

/**
 * Follows what the API sends over a WebSocket (ADR 0026), keeping at most
 * `limit` of it. It resumes from the last cursor after falling behind or
 * losing the connection, gives up after the connection fails
 * `RETRY_DELAYS_MS.length` times in a row, and ends when the API closes
 * the socket normally.
 */
export function useTail<Item, Message extends TailMessage>(
  connect: (after?: string) => TailSocket,
  options: TailOptions<Item, Message>,
) {
  const state = ref<TailState>('idle')
  const items = shallowRef<Item[]>([])
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

  function keep(arrived: readonly Item[]) {
    if (arrived.length === 0) {
      return
    }
    items.value = options.newestFirst
      ? [...[...arrived].reverse(), ...items.value].slice(0, options.limit)
      : [...items.value, ...arrived].slice(-options.limit)
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
      const message = JSON.parse(String(event.data)) as Message
      keep(options.items(message))
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
    current.onclose = (event: CloseEvent) => {
      if (socket !== current) {
        return
      }
      socket = undefined
      if (event.code === NORMAL_CLOSURE) {
        state.value = 'ended'
        return
      }
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
    items.value = []
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
  return { state, items, failure, start, stop }
}
