import { http, HttpResponse, ws, type AnyHandler } from 'msw'
import type {
  LogDeletionItem,
  LogDeletionList,
  LogDeletionRequest,
  LogPageResponse,
  LogRecordItem,
  LogTailMessage,
} from '@/api/generated'

const SITES = ['shop', 'docs', 'blog'] as const
const REQUESTS = [
  ['GET', '/'],
  ['GET', '/cart'],
  ['POST', '/api/orders'],
  ['GET', '/assets/app.js'],
  ['GET', '/docs/install'],
  ['PUT', '/api/cart/items'],
] as const
/** Sample records are this far apart. */
const SPACING_MS = 37_000
const SAMPLES = 400
/** How often a followed tail receives a record. */
const TAIL_INTERVAL_MS = 1_500

function pick<T>(items: readonly T[], index: number): T {
  return items[index % items.length] as T
}

/** The `index`th record, in the gateway's JSON access and error log formats. */
function sample(at: Date, index: number): LogRecordItem {
  const site = pick(SITES, index)
  const request_id = `01926f3a-${index.toString(16).padStart(4, '0')}-7c3d-9e2f-5a1b8c4d6e7f`
  const common = {
    timestamp: at.toISOString(),
    'pingora_panel.site.id': site,
    'pingora_panel.route.id': 'app',
    'pingora_panel.request.id': request_id,
    'server.address': `${site}.example`,
  }
  if (index % 23 === 7) {
    const line = {
      ...common,
      'event.name': 'pingora_panel.error',
      severity_text: 'ERROR',
      message: 'connecting to the upstream failed: connection refused',
      'error.type': 'ConnectRefused',
    }
    return {
      time: at.toISOString(),
      kind: 'error',
      line: JSON.stringify(line),
      site,
      route: 'app',
      request_id,
      fields: { severity_text: 'ERROR', error_type: 'ConnectRefused' },
    }
  }
  const [method, path] = pick(REQUESTS, index)
  const status = index % 31 === 5 ? 502 : index % 11 === 3 ? 404 : index % 13 === 4 ? 304 : 200
  const client = `203.0.113.${((index * 7) % 250) + 1}`
  const line = {
    ...common,
    'event.name': 'pingora_panel.access',
    'http.request.method': method,
    'url.path': path,
    'http.response.status_code': status,
    'client.address': client,
    'http.server.request.duration': Number((0.004 + (index % 9) * 0.011).toFixed(3)),
  }
  return {
    time: at.toISOString(),
    kind: 'access',
    line: JSON.stringify(line),
    site,
    route: 'app',
    status,
    method,
    path,
    client,
    request_id,
    fields: {
      http_request_method: method,
      url_path: path,
      http_response_status_code: String(status),
      client_address: client,
    },
  }
}

function matches(record: LogRecordItem, query: URLSearchParams): boolean {
  const status = query.get('status')
  const path = query.get('path')
  const text = query.get('text')?.toLowerCase()
  return (
    (!query.get('kind') || record.kind === query.get('kind')) &&
    (!query.get('site') || record.site === query.get('site')) &&
    (!query.get('route') || record.route === query.get('route')) &&
    (!status || String(record.status ?? '').startsWith(status.replace(/x+$/i, ''))) &&
    (!query.get('client') || record.client === query.get('client')) &&
    (!path || (record.path ?? '').startsWith(path)) &&
    (!query.get('request_id') || record.request_id === query.get('request_id')) &&
    (!text || record.line.toLowerCase().includes(text))
  )
}

/** Records before `until`, newest first. */
function records(query: URLSearchParams): LogRecordItem[] {
  const until = Date.parse(query.get('until') ?? '') || Date.now()
  const since = Date.parse(query.get('since') ?? '') || until - 3_600_000
  const newest = Math.floor(Date.now() / SPACING_MS)
  return Array.from({ length: SAMPLES }, (_, step) => {
    const index = newest - step
    return sample(new Date(index * SPACING_MS), index)
  }).filter((record) => {
    const time = Date.parse(record.time)
    return time < until && time >= since && matches(record, query)
  })
}

/** The logs API stand-in: sampled records, deletions and a tail that keeps arriving. */
export function logHandlers(): AnyHandler[] {
  const deletions: LogDeletionItem[] = [
    {
      site: null,
      since: new Date(0).toISOString(),
      until: new Date(Date.now() - 7 * 86_400_000).toISOString(),
      requested_at: new Date(Date.now() - 7 * 86_400_000).toISOString(),
      state: 'applied',
    },
  ]
  const tail = ws.link(/\/api\/v1\/logs\/tail/)
  return [
    http.get('*/api/v1/logs', ({ request }) => {
      const query = new URL(request.url).searchParams
      const limit = Number(query.get('limit')) || 100
      const found = records(query)
      const page = found.slice(0, limit)
      return HttpResponse.json({
        records: page,
        next_until: found.length > limit ? (page.at(-1)?.time ?? null) : null,
      } satisfies LogPageResponse)
    }),
    http.get('*/api/v1/logs/download', ({ request }) => {
      const lines = records(new URL(request.url).searchParams).map((record) => `${record.line}\n`)
      return new HttpResponse(lines.join(''), {
        headers: {
          'content-type': 'text/plain; charset=utf-8',
          'content-disposition': 'attachment; filename="gateway.log"',
        },
      })
    }),
    http.get('*/api/v1/logs/deletions', () =>
      HttpResponse.json({ deletions } satisfies LogDeletionList),
    ),
    http.post('*/api/v1/logs/deletions', async ({ request }) => {
      const asked = (await request.json()) as LogDeletionRequest
      const now = new Date().toISOString()
      const deletion: LogDeletionItem = {
        site: asked.site ?? null,
        since: asked.since ?? new Date(0).toISOString(),
        until: now,
        requested_at: now,
        state: 'pending',
      }
      deletions.unshift(deletion)
      return HttpResponse.json(deletion, { status: 202 })
    }),
    tail.addEventListener('connection', ({ client }) => {
      const query = new URL(client.url).searchParams
      let index = Math.floor(Date.now() / SPACING_MS) * 10
      const timer = setInterval(() => {
        const record = sample(new Date(), (index += 1))
        const message: LogTailMessage = {
          records: matches(record, query) ? [record] : [],
          cursor: record.time,
        }
        client.send(JSON.stringify(message))
      }, TAIL_INTERVAL_MS)
      client.addEventListener('close', () => clearInterval(timer))
    }),
  ]
}
