import type { LogRecordItem } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'
import { apiUrl, websocketUrl, type ApiLocation } from '@/lib/api'

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

/** How many followed records are kept; older ones drop off. */
export const TAIL_LIMIT = 1_000

/** The tail of what `query` matches, with the WebSocket scheme. */
export function tailUrl(query: Record<string, string>, at: ApiLocation): string {
  return websocketUrl('/api/v1/logs/tail', query, at)
}

/** The log file of what `query` matches, for the browser to save. */
export function downloadUrl(query: Record<string, string>, at: ApiLocation): string {
  return apiUrl('/api/v1/logs/download', query, at).toString()
}
