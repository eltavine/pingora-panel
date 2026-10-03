import type {
  Action,
  MatchKind,
  Route,
  RouteInput,
  SiteInput,
  SiteKind,
  SiteView,
  WwwRedirect,
} from '@/api/generated'
import { parseHosts } from './presentation'

export type ActionType = Action['type']

export const ACTION_TYPES: readonly ActionType[] = ['proxy', 'static', 'redirect', 'respond']
export const SITE_KINDS: readonly SiteKind[] = [
  'reverse_proxy',
  'static',
  'redirect',
  'maintenance',
]
export const MATCH_KINDS: readonly MatchKind[] = ['prefix', 'exact', 'glob', 'regex']
export const REDIRECT_STATUSES = [301, 302, 303, 307, 308] as const
export const WWW_REDIRECTS: readonly WwwRedirect[] = ['none', 'add_www', 'remove_www']

export const siteKindAction: Record<SiteKind, ActionType> = {
  reverse_proxy: 'proxy',
  static: 'static',
  redirect: 'redirect',
  maintenance: 'respond',
}

/** Every action variant's fields at once, so switching type keeps typed input. */
export interface ActionForm {
  type: ActionType
  upstreamId: string
  root: string
  indexFiles: string
  spaFallback: boolean
  location: string
  redirectStatus: number
  preservePath: boolean
  respondStatus: number | string
  body: string
  contentType: string
  retryAfter: number | string
}

/** Strict-Transport-Security as the form edits it, in days. */
export interface HstsForm {
  enabled: boolean
  maxAgeDays: number | string
  includeSubdomains: boolean
  preload: boolean
}

const DAY_SECONDS = 86_400
/** Browsers' preload lists accept a year or more. */
export const PRELOAD_DAYS = 365

export interface SiteForm {
  name: string
  action: ActionForm
  domains: string
  httpsRedirect: boolean
  hsts: HstsForm
  wwwRedirect: WwwRedirect
  listenerIds: string[]
  tlsProfileId: string
  group: string
  tags: string
  note: string
}

export interface RouteForm {
  name: string
  enabled: boolean
  priority: number | string
  kind: MatchKind
  path: string
  host: string
  action: ActionForm
}

/** Empty text is absent, not an empty value. */
export function optionalText(value: string): string | null {
  const trimmed = value.trim()
  return trimmed === '' ? null : trimmed
}

/** Empty or non-numeric input is absent. */
export function optionalNumber(value: number | string | null | undefined): number | null {
  if (value === null || value === undefined || value === '') {
    return null
  }
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

export function splitList(value: string): string[] {
  return value
    .split(',')
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
}

export function actionForm(action?: Action): ActionForm {
  const form: ActionForm = {
    type: action?.type ?? 'proxy',
    upstreamId: '',
    root: '',
    indexFiles: 'index.html',
    spaFallback: false,
    location: '',
    redirectStatus: 308,
    preservePath: true,
    respondStatus: 503,
    body: '',
    contentType: '',
    retryAfter: '',
  }
  switch (action?.type) {
    case 'proxy':
      form.upstreamId = action.upstream_id
      break
    case 'static':
      form.root = action.root
      form.indexFiles = (action.index_files ?? []).join(', ')
      form.spaFallback = action.spa_fallback ?? false
      break
    case 'redirect':
      form.location = action.location
      form.redirectStatus = action.status ?? 308
      form.preservePath = action.preserve_path ?? true
      break
    case 'respond':
      form.respondStatus = action.status ?? 503
      form.body = action.body ?? ''
      form.contentType = action.content_type ?? ''
      form.retryAfter = action.retry_after_seconds ?? ''
      break
  }
  return form
}

export function toAction(form: ActionForm): Action {
  switch (form.type) {
    case 'proxy':
      return { type: 'proxy', upstream_id: form.upstreamId }
    case 'static':
      return {
        type: 'static',
        root: form.root.trim(),
        index_files: splitList(form.indexFiles),
        spa_fallback: form.spaFallback,
      }
    case 'redirect':
      return {
        type: 'redirect',
        location: form.location.trim(),
        status: form.redirectStatus,
        preserve_path: form.preservePath,
      }
    case 'respond':
      return {
        type: 'respond',
        status: optionalNumber(form.respondStatus) ?? 503,
        body: form.body === '' ? null : form.body,
        content_type: optionalText(form.contentType),
        retry_after_seconds: optionalNumber(form.retryAfter),
      }
  }
}

export function siteForm(site?: SiteView): SiteForm {
  return {
    name: site?.name ?? '',
    action: actionForm(site?.action),
    domains: '',
    httpsRedirect: site?.https_redirect ?? false,
    hsts: {
      enabled: Boolean(site?.hsts),
      maxAgeDays: site?.hsts ? Math.round(site.hsts.max_age_seconds / DAY_SECONDS) : PRELOAD_DAYS,
      includeSubdomains: site?.hsts?.include_subdomains ?? false,
      preload: site?.hsts?.preload ?? false,
    },
    wwwRedirect: site?.www_redirect ?? 'none',
    listenerIds: [...(site?.listener_ids ?? [])],
    tlsProfileId: site?.tls_profile_id ?? '',
    group: site?.group ?? '',
    tags: (site?.tags ?? []).join(', '),
    note: site?.note ?? '',
  }
}

/**
 * The site as the API stores it. Editing keeps the domains, routes and
 * flags that this form does not show, because replacement is total.
 */
export function siteInput(form: SiteForm, site?: SiteView): SiteInput {
  const hosts = parseHosts(form.domains)
  return {
    name: form.name.trim(),
    action: toAction(form.action),
    enabled: site?.enabled ?? true,
    favorite: site?.favorite ?? false,
    domains: site
      ? site.domains
      : hosts.map((host, index) => ({ host, primary: index === 0, enabled: true })),
    routes: site ? (site.routes ?? []).map(routeInputOf) : [],
    https_redirect: form.httpsRedirect,
    hsts: form.hsts.enabled
      ? {
          max_age_seconds: (optionalNumber(form.hsts.maxAgeDays) ?? 0) * DAY_SECONDS,
          include_subdomains: form.hsts.includeSubdomains,
          preload: form.hsts.preload,
        }
      : null,
    www_redirect: form.wwwRedirect,
    listener_ids: form.listenerIds,
    tls_profile_id: optionalText(form.tlsProfileId),
    group: optionalText(form.group),
    tags: splitList(form.tags),
    note: optionalText(form.note),
  }
}

export function routeInputOf(route: Route): RouteInput {
  return {
    id: route.id,
    name: route.name,
    enabled: route.enabled,
    priority: route.priority,
    match: route.match,
    action: route.action,
  }
}

export function routeForm(route: Route | undefined, priority: number): RouteForm {
  return {
    name: route?.name ?? '',
    enabled: route?.enabled ?? true,
    priority: route?.priority ?? priority,
    kind: route?.match.kind ?? 'prefix',
    path: route?.match.path ?? '/',
    host: route?.match.host ?? '',
    action: actionForm(route?.action),
  }
}

export function routeInput(form: RouteForm, id?: string): RouteInput {
  return {
    id: id ?? null,
    name: optionalText(form.name),
    enabled: form.enabled,
    priority: optionalNumber(form.priority) ?? 0,
    match: { kind: form.kind, path: form.path.trim(), host: optionalText(form.host) },
    action: toAction(form.action),
  }
}

/** Priority for a new route: after every existing one, in steps of ten. */
export function nextPriority(routes: readonly Route[]): number {
  return routes.reduce((highest, route) => Math.max(highest, route.priority), 0) + 10
}
