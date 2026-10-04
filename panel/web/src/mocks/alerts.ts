import { http, HttpResponse, type AnyHandler } from 'msw'
import type {
  AlertChannelSecretView,
  AlertChannelView,
  AlertNotificationView,
  AlertRuleSpecBody,
  AlertRuleView,
  NewAlertChannelBody,
} from '@/api/generated'

function now(offsetMinutes = 0): string {
  return new Date(Date.now() + offsetMinutes * 60_000).toISOString()
}

function rule(id: string, spec: AlertRuleSpecBody, version = 1): AlertRuleView {
  return {
    id,
    spec,
    version,
    etag: `"${version}"`,
    created_at: now(-7 * 24 * 60),
    updated_at: now(-60),
    state: 'inactive',
    evaluated_at: now(),
  }
}

/** The alerts API stand-in: rules, one firing, a webhook and its notifications. */
export function alertHandlers(): AnyHandler[] {
  const rules = new Map<string, AlertRuleView>([
    [
      'shop-errors',
      {
        ...rule('shop-errors', {
          name: 'Shop errors',
          description: 'Too many requests to the shop fail.',
          measure: 'server_error_ratio',
          comparison: 'above',
          threshold: 0.05,
          pending_seconds: 300,
          site: 'shop',
          severity: 'critical',
          enabled: true,
          channels: ['ops'],
        }),
        state: 'firing',
        since: now(-12),
        value: 0.083,
      },
    ],
    [
      'slow-docs',
      {
        ...rule('slow-docs', {
          name: 'Slow docs',
          description: '',
          measure: 'latency_p95',
          comparison: 'above',
          threshold: 1.5,
          pending_seconds: 600,
          site: 'docs',
          severity: 'warning',
          enabled: true,
          channels: ['ops'],
        }),
        value: 0.42,
      },
    ],
  ])
  const channels = new Map<string, AlertChannelView>([
    [
      'ops',
      {
        id: 'ops',
        kind: 'webhook',
        target: 'https://hooks.example',
        version: 1,
        etag: '"1"',
        created_at: now(-30 * 24 * 60),
        updated_at: now(-30 * 24 * 60),
      },
    ],
  ])
  const notifications: AlertNotificationView[] = [
    {
      id: '01926f3a-0001-7c3d-9e2f-5a1b8c4d6e7f',
      rule: 'shop-errors',
      channel: 'ops',
      kind: 'firing',
      state: 'delivered',
      attempts: 1,
      created_at: now(-12),
      delivered_at: now(-12),
    },
  ]
  const secret = (channel: AlertChannelView): AlertChannelSecretView => ({
    channel,
    secret: `whsec_${btoa(crypto.randomUUID()).slice(0, 43)}`,
  })
  return [
    http.get('*/api/v1/alert-rules', () => HttpResponse.json([...rules.values()])),
    http.put('*/api/v1/alert-rules/:id', async ({ params, request }) => {
      const id = String(params.id)
      const spec = (await request.json()) as AlertRuleSpecBody
      const current = rules.get(id)
      const saved = {
        ...(current ?? rule(id, spec)),
        ...rule(id, spec, (current?.version ?? 0) + 1),
      }
      rules.set(id, saved)
      return HttpResponse.json(saved, {
        status: current ? 200 : 201,
        headers: { etag: saved.etag },
      })
    }),
    http.delete('*/api/v1/alert-rules/:id', ({ params }) => {
      rules.delete(String(params.id))
      return new HttpResponse(null, { status: 204 })
    }),
    http.get('*/api/v1/alert-channels', () => HttpResponse.json([...channels.values()])),
    http.post('*/api/v1/alert-channels', async ({ request }) => {
      const body = (await request.json()) as NewAlertChannelBody
      const channel: AlertChannelView = {
        id: body.id,
        kind: 'webhook',
        target: new URL(body.url).origin,
        version: 1,
        etag: '"1"',
        created_at: now(),
        updated_at: now(),
      }
      channels.set(channel.id, channel)
      return HttpResponse.json(secret(channel), { status: 201 })
    }),
    http.post('*/api/v1/alert-channels/:id/rotate', ({ params }) => {
      const current = channels.get(String(params.id))
      if (!current) {
        return new HttpResponse(null, { status: 404 })
      }
      const version = current.version + 1
      const rotated = { ...current, version, etag: `"${version}"`, updated_at: now() }
      channels.set(rotated.id, rotated)
      return HttpResponse.json(secret(rotated))
    }),
    http.post('*/api/v1/alert-channels/:id/test', () =>
      HttpResponse.json({ delivered: true, status: 200 }),
    ),
    http.delete('*/api/v1/alert-channels/:id', ({ params }) => {
      channels.delete(String(params.id))
      return new HttpResponse(null, { status: 204 })
    }),
    http.get('*/api/v1/alert-notifications', ({ request }) => {
      const wanted = new URL(request.url).searchParams.get('rule')
      return HttpResponse.json(
        notifications.filter((notification) => !wanted || notification.rule === wanted),
      )
    }),
  ]
}
