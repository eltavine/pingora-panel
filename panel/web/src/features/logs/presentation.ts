import type { LogRecordItem } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** The filters searches, tails and downloads share, by their query names. */
export const FILTERS = [
  'kind',
  'site',
  'route',
  'status',
  'client',
  'path',
  'request_id',
  'text',
] as const

/** Only the values that are set, as query parameters. */
export function queryOf(values: Record<string, string | undefined>): Record<string, string> {
  const query: Record<string, string> = {}
  for (const [name, value] of Object.entries(values)) {
    if (value) {
      query[name] = value
    }
  }
  return query
}

/** Server errors and error records read as negative, client errors as warnings. */
export function toneOf(record: LogRecordItem): StatusTone {
  const status = record.status ?? 0
  if (record.kind === 'error' || status >= 500) {
    return 'negative'
  }
  if (status >= 400) {
    return 'warning'
  }
  return status > 0 ? 'positive' : 'neutral'
}

/** The message of an error record: from its JSON line, else from its fields. */
export function messageOf(record: LogRecordItem): string {
  try {
    const parsed: unknown = JSON.parse(record.line)
    if (typeof parsed === 'object' && parsed !== null && 'message' in parsed) {
      return String(parsed.message)
    }
  } catch {
    // Combined lines are not JSON.
  }
  return record.fields.message ?? record.line
}

/** What a record says in a line: an access record's request, an error record's message. */
export function summaryOf(record: LogRecordItem): string {
  return record.kind === 'access'
    ? [record.method, record.path].filter(Boolean).join(' ') || record.line
    : messageOf(record)
}

/** Where the API answers `path`, as the generated client joins them. */
export interface ApiLocation {
  /** The client's base URL; empty when the API shares the console's origin. */
  baseUrl: string
  /** The console page the base URL is relative to. */
  page: string
}

function endpoint(path: string, query: Record<string, string>, at: ApiLocation): URL {
  const url = new URL(`${at.baseUrl}${path}`, at.page)
  for (const [name, value] of Object.entries(query)) {
    url.searchParams.set(name, value)
  }
  return url
}

/** The tail of what `query` matches, with the WebSocket scheme. */
export function tailUrl(query: Record<string, string>, at: ApiLocation): string {
  const url = endpoint('/api/v1/logs/tail', query, at)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url.toString()
}

/** The log file of what `query` matches, for the browser to save. */
export function downloadUrl(query: Record<string, string>, at: ApiLocation): string {
  return endpoint('/api/v1/logs/download', query, at).toString()
}
