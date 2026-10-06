import type { HeaderLine, LuaRunOutcome, LuaScriptInfo } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** The phases a script can be tested in, in the order a connection and its requests run them. */
export const PHASES = [
  'ssl_client_hello',
  'ssl_session_fetch',
  'ssl_cert',
  'ssl_session_store',
  'set',
  'server_rewrite',
  'rewrite',
  'access',
  'precontent',
  'content',
  'balancer',
  'proxy_ssl_cert',
  'header_filter',
  'body_filter',
  'log',
] as const

export type Phase = (typeof PHASES)[number]

export const METHODS = ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'] as const

/** How a run's outcome reads; a failure is the only negative one. */
export function outcomeTone(outcome: LuaRunOutcome): StatusTone {
  switch (outcome) {
    case 'failed':
      return 'negative'
    case 'abort':
      return 'warning'
    case 'respond':
      return 'positive'
    default:
      return 'neutral'
  }
}

/** The first twelve hexadecimal digits of a script's SHA-256. */
export function shortVersion(sha256: string): string {
  return sha256.slice(0, 12)
}

/** Whether the script is a file the console can edit, rather than code written in a block. */
export function isFile(script: LuaScriptInfo): boolean {
  return script.id === script.file && script.file.endsWith('.lua')
}

/** The handlers a library's scripts are used by. */
export function handlerCount(scripts: readonly LuaScriptInfo[]): number {
  return scripts.reduce((count, script) => count + script.uses.length, 0)
}

/**
 * Header lines as `Name: value` text, one per line; lines without a colon
 * are reported by their number.
 */
export function parseHeaders(text: string): { headers: HeaderLine[]; invalid: number[] } {
  const headers: HeaderLine[] = []
  const invalid: number[] = []
  text.split('\n').forEach((line, index) => {
    if (!line.trim()) {
      return
    }
    const colon = line.indexOf(':')
    if (colon <= 0) {
      invalid.push(index + 1)
      return
    }
    headers.push({ name: line.slice(0, colon).trim(), value: line.slice(colon + 1).trim() })
  })
  return { headers, invalid }
}

/** Microseconds as people read them. */
export function duration(micros: number): string {
  if (micros < 1000) {
    return `${micros} µs`
  }
  return `${(micros / 1000).toFixed(micros < 10_000 ? 2 : 1)} ms`
}
