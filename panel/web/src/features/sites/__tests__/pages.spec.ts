import { describe, expect, it } from 'vitest'
import type { ErrorPage, Route, SiteView } from '@/api/generated'
import { routeForm, routeInput, routeInputOf, siteForm, siteInput } from '../forms'
import {
  describePages,
  faviconForm,
  faviconProblem,
  isNetwork,
  maintenanceForm,
  maintenanceProblem,
  newPage,
  pageForm,
  pageProblem,
  pagesForm,
  robotsForm,
  robotsProblem,
  routePagesMode,
  toFavicon,
  toMaintenance,
  toPage,
  toRobots,
} from '../pages'

const pages: ErrorPage[] = [
  {
    statuses: [404, 410],
    response: { kind: 'body', body: '<h1>No $uri here</h1>', content_type: null },
    status: null,
  },
  {
    statuses: [502, 503],
    response: { kind: 'file', path: 'errors/50x.html' },
    status: 503,
  },
  {
    statuses: [403],
    response: { kind: 'redirect', location: 'https://$host/', status: 301 },
  },
]

describe('error page forms', () => {
  it('round-trip every kind of page', () => {
    for (const page of pages) {
      expect(toPage(pageForm(page))).toEqual(page)
    }
    expect(newPage(503).statuses).toBe('503')
    expect(describePages({ pages })).toEqual([403, 404, 410, 502, 503])
  })

  it('point out what keeps a page from being saved', () => {
    const problem = (statuses: string, change: Partial<ReturnType<typeof newPage>> = {}) =>
      pageProblem({ ...newPage(), statuses, body: 'x', ...change })?.key
    expect(problem('')).toBe('sites.errorPages.problems.statusesRequired')
    expect(problem('404, abc')).toBe('sites.errorPages.problems.statusShape')
    expect(problem('302')).toBe('sites.errorPages.problems.statusRange')
    expect(problem('404 404')).toBe('sites.errorPages.problems.repeated')
    expect(problem('404', { body: ' ' })).toBe('sites.errorPages.problems.bodyRequired')
    for (const path of ['', '/abs/404.html', 'a/../404.html', 'a//b.html']) {
      expect(problem('404', { kind: 'file', path })).toBe('sites.errorPages.problems.pathShape')
    }
    expect(problem('404', { kind: 'redirect' })).toBe('sites.errorPages.problems.locationRequired')
    expect(problem('404', { status: 700 })).toBe('sites.errorPages.problems.statusOverride')
    const earlier = { ...newPage(404), body: 'x' }
    expect(pageProblem({ ...newPage(404), body: 'y' }, [earlier])).toEqual({
      key: 'sites.errorPages.problems.repeated',
      values: { status: 404 },
    })
    for (const page of pages) {
      expect(pageProblem(pageForm(page))).toBeUndefined()
    }
  })

  it("let routes take their site's pages, their own, or none", () => {
    expect(routePagesMode(undefined)).toBe('site')
    expect(routePagesMode({ pages: [] })).toBe('none')
    expect(routePagesMode({ pages })).toBe('own')
    const form = routeForm(undefined, 10)
    expect(routeInput(form).error_pages).toBeNull()
    form.errorPages = pagesForm({ pages, intercept: true })
    form.pagesMode = 'own'
    expect(routeInput(form).error_pages).toEqual({ pages, intercept: true })
    form.pagesMode = 'none'
    expect(routeInput(form).error_pages).toEqual({ pages: [], intercept: false })
  })

  it("keep routes' pages when a site is replaced", () => {
    const route = {
      id: 'r',
      priority: 10,
      match: { kind: 'prefix', path: '/api' },
      action: { type: 'respond', status: 204 },
      error_pages: { pages, intercept: true },
    } as Route
    expect(routeInputOf(route).error_pages).toEqual({ pages, intercept: true })
    expect(routeInputOf({ ...route, error_pages: undefined }).error_pages).toBeNull()
  })
})

describe('maintenance forms', () => {
  it('send nothing for a site that never had maintenance', () => {
    expect(toMaintenance(maintenanceForm())).toBeNull()
    const kept = { enabled: true, status: 503, allow: ['10.0.0.0/8'], retry_after_seconds: 60 }
    const off = { ...maintenanceForm(kept), enabled: false }
    expect(toMaintenance(off, kept)).toEqual({
      enabled: false,
      allow: ['10.0.0.0/8'],
      status: 503,
      body: null,
      content_type: null,
      retry_after_seconds: 60,
    })
  })

  it('reach the site input with the rest of its answers', () => {
    const site = { name: 'shop', action: { type: 'respond', status: 204 } } as SiteView
    const form = siteForm(site)
    form.maintenance.enabled = true
    form.maintenance.allow = '10.0.0.0/8\n2001:db8::1'
    form.robots.choice = 'disallow_all'
    form.favicon = { choice: 'no_content', path: '', location: '' }
    form.errorPages = pagesForm({ pages })
    const input = siteInput(form, site)
    expect(input.maintenance?.allow).toEqual(['10.0.0.0/8', '2001:db8::1'])
    expect(input.robots).toEqual({ kind: 'disallow_all' })
    expect(input.favicon).toEqual({ kind: 'no_content' })
    expect(input.error_pages?.pages).toEqual(pages)
  })

  it('check allowlists line by line', () => {
    for (const network of ['10.0.0.0/8', '192.0.2.7', '2001:db8::/32', '::1', 'fe80::1/128']) {
      expect(isNetwork(network)).toBe(true)
    }
    for (const network of [
      'office',
      '10.0.0.0/33',
      '256.0.0.1',
      '2001:db8::/129',
      '1.2.3',
      '::g',
    ]) {
      expect(isNetwork(network)).toBe(false)
    }
    const form = { ...maintenanceForm(), enabled: true, allow: '10.0.0.0/8\noffice' }
    expect(maintenanceProblem(form)).toEqual({
      key: 'sites.maintenance.problems.network',
      values: { line: 2 },
    })
    expect(maintenanceProblem({ ...form, allow: '', status: 99 })?.key).toBe(
      'sites.maintenance.problems.status',
    )
  })
})

describe('robots.txt and favicon forms', () => {
  it('round-trip each answer and leave the paths to routes when unset', () => {
    expect(toRobots(robotsForm())).toBeNull()
    expect(toRobots(robotsForm({ kind: 'allow_all' }))).toEqual({ kind: 'allow_all' })
    const custom = { kind: 'custom' as const, body: 'User-agent: *\nDisallow: /cart\n' }
    expect(toRobots(robotsForm(custom))).toEqual(custom)
    expect(robotsProblem({ choice: 'custom', body: ' ' })).toBe(
      'sites.robots.problems.bodyRequired',
    )
    expect(toFavicon(faviconForm())).toBeNull()
    for (const favicon of [
      { kind: 'no_content' as const },
      { kind: 'file' as const, path: 'shop/favicon.ico' },
      { kind: 'redirect' as const, location: 'https://cdn.example.com/icon.png' },
    ]) {
      expect(toFavicon(faviconForm(favicon))).toEqual(favicon)
      expect(faviconProblem(faviconForm(favicon))).toBeUndefined()
    }
  })

  it('want favicon files in a directory and redirects to an address', () => {
    for (const path of ['favicon.ico', 'shop/../favicon.ico', 'shop/fav icon.ico', '/shop/x.ico']) {
      expect(faviconProblem({ choice: 'file', path, location: '' })).toBe(
        'sites.favicon.problems.pathShape',
      )
    }
    for (const location of ['cdn.example.com/x.png', '//cdn.example.com/x.png', 'https://']) {
      expect(faviconProblem({ choice: 'redirect', path: '', location })).toBe(
        'sites.favicon.problems.locationShape',
      )
    }
  })
})
