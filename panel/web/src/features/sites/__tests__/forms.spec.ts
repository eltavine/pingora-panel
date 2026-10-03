import { describe, expect, it } from 'vitest'
import type { Action, Route, SiteView } from '@/api/generated'
import {
  actionForm,
  nextPriority,
  routeForm,
  routeInput,
  siteForm,
  siteInput,
  toAction,
} from '../forms'

const route: Route = {
  id: '6f2c0d2e-7c4b-4bb0-9a43-2c9a4b1c0d11',
  name: 'api',
  enabled: true,
  priority: 10,
  match: { kind: 'prefix', path: '/api/', host: null },
  action: { type: 'proxy', upstream_id: 'b9c1' },
}

const site: SiteView = {
  id: '0d7c',
  name: 'Shop',
  action: { type: 'static', root: 'shop', index_files: ['index.html'], spa_fallback: true },
  enabled: false,
  favorite: true,
  domains: [{ host: 'shop.example', primary: true, enabled: true }],
  routes: [route],
  tags: ['prod', 'eu'],
  group: 'retail',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-02T00:00:00Z',
  etag: '"abc"',
  https: false,
  kind: 'static',
  status: 'stopped',
}

describe('action forms', () => {
  it('round-trips every action type', () => {
    const actions: Action[] = [
      { type: 'proxy', upstream_id: 'u1' },
      {
        type: 'static',
        root: 'docs',
        index_files: ['index.html', 'index.htm'],
        spa_fallback: true,
      },
      { type: 'redirect', location: 'https://example.com', status: 301, preserve_path: false },
      {
        type: 'respond',
        status: 503,
        body: 'down',
        content_type: 'text/plain',
        retry_after_seconds: 60,
      },
    ]
    for (const action of actions) {
      expect(toAction(actionForm(action))).toEqual(action)
    }
  })

  it('treats empty optional response fields as absent', () => {
    const form = actionForm({ type: 'respond' })
    expect(toAction(form)).toEqual({
      type: 'respond',
      status: 503,
      body: null,
      content_type: null,
      retry_after_seconds: null,
    })
  })
})

describe('site forms', () => {
  it('makes the first new domain primary', () => {
    const form = siteForm()
    form.name = ' Blog '
    form.action.upstreamId = 'u1'
    form.domains = 'blog.example\n# staging\nwww.blog.example, '
    const input = siteInput(form)
    expect(input.name).toBe('Blog')
    expect(input.domains).toEqual([
      { host: 'blog.example', primary: true, enabled: true },
      { host: 'www.blog.example', primary: false, enabled: true },
    ])
  })

  it('keeps what the form does not show when replacing a site', () => {
    const form = siteForm(site)
    form.tags = 'prod, , eu, new'
    const input = siteInput(form, site)
    expect(input.enabled).toBe(false)
    expect(input.favorite).toBe(true)
    expect(input.domains).toEqual(site.domains)
    expect(input.routes).toEqual([route])
    expect(input.tags).toEqual(['prod', 'eu', 'new'])
    expect(input.tls_profile_id).toBeNull()
  })
})

describe('route forms', () => {
  it('keeps the route identity and drops an empty host', () => {
    const form = routeForm(route, 0)
    form.path = ' /v2/ '
    expect(routeInput(form, route.id)).toEqual({
      ...route,
      match: { kind: 'prefix', path: '/v2/', host: null },
    })
  })

  it('places new routes after existing ones', () => {
    expect(nextPriority([])).toBe(10)
    expect(nextPriority([route, { ...route, priority: 35 }])).toBe(45)
    expect(routeForm(undefined, 45).priority).toBe(45)
  })
})
