import type {
  ErrorPage,
  ErrorPages,
  ErrorResponse,
  Favicon,
  Robots,
  SiteMaintenance,
} from '@/api/generated'
import { optionalNumber, optionalText } from '@/lib/forms'

export type PageKind = ErrorResponse['kind']

export const PAGE_KINDS: readonly PageKind[] = ['body', 'file', 'redirect']
/** The errors operators give pages first. */
export const PAGE_PRESETS = [404, 403, 502, 503] as const
export const PAGE_REDIRECT_STATUSES = [302, 301, 303, 307, 308] as const

/** Every page kind's fields at once, so switching kind keeps typed input. */
export interface PageForm {
  /** Comma-separated, such as `502, 503`. */
  statuses: string
  kind: PageKind
  body: string
  contentType: string
  path: string
  location: string
  redirectStatus: number
  /** The status a body or file answers with instead of the error's. */
  status: number | string
}

export interface PagesForm {
  pages: PageForm[]
  intercept: boolean
}

/** A route answers its errors with its site's pages, its own, or none. */
export type RoutePagesMode = 'site' | 'own' | 'none'
export const ROUTE_PAGES_MODES: readonly RoutePagesMode[] = ['site', 'own', 'none']

export function newPage(status?: number): PageForm {
  return {
    statuses: status === undefined ? '' : String(status),
    kind: 'body',
    body: '',
    contentType: '',
    path: '',
    location: '',
    redirectStatus: 302,
    status: '',
  }
}

export function pageForm(page: ErrorPage): PageForm {
  const form = newPage()
  form.statuses = page.statuses.join(', ')
  form.kind = page.response.kind
  form.status = page.status ?? ''
  switch (page.response.kind) {
    case 'body':
      form.body = page.response.body
      form.contentType = page.response.content_type ?? ''
      break
    case 'file':
      form.path = page.response.path
      break
    case 'redirect':
      form.location = page.response.location
      form.redirectStatus = page.response.status ?? 302
      break
  }
  return form
}

/** The statuses written, or undefined when one is not a whole number. */
export function parseStatuses(value: string): number[] | undefined {
  const parts = value
    .split(/[\s,]+/)
    .map((part) => part.trim())
    .filter(Boolean)
  if (!parts.every((part) => /^\d+$/.test(part))) return undefined
  return parts.map(Number)
}

export function toPage(form: PageForm): ErrorPage {
  const statuses = parseStatuses(form.statuses) ?? []
  switch (form.kind) {
    case 'body':
      return {
        statuses,
        response: {
          kind: 'body',
          body: form.body,
          content_type: optionalText(form.contentType),
        },
        status: optionalNumber(form.status),
      }
    case 'file':
      return {
        statuses,
        response: { kind: 'file', path: form.path.trim() },
        status: optionalNumber(form.status),
      }
    case 'redirect':
      return {
        statuses,
        response: { kind: 'redirect', location: form.location.trim(), status: form.redirectStatus },
      }
  }
}

export function pagesForm(pages?: ErrorPages | null): PagesForm {
  return { pages: (pages?.pages ?? []).map(pageForm), intercept: pages?.intercept ?? false }
}

export function toPages(form: PagesForm): ErrorPages {
  return { pages: form.pages.map(toPage), intercept: form.intercept }
}

/**
 * What keeps a page from being saved, as a message key with its values;
 * `others` are the pages before it, whose statuses it may not repeat.
 */
export function pageProblem(
  form: PageForm,
  others: readonly PageForm[] = [],
): { key: string; values?: Record<string, unknown> } | undefined {
  const statuses = parseStatuses(form.statuses)
  if (statuses === undefined) return { key: 'sites.errorPages.problems.statusShape' }
  if (statuses.length === 0) return { key: 'sites.errorPages.problems.statusesRequired' }
  const outside = statuses.find((status) => status < 400 || status > 599)
  if (outside !== undefined) {
    return { key: 'sites.errorPages.problems.statusRange', values: { status: outside } }
  }
  const taken = new Set(others.flatMap((other) => parseStatuses(other.statuses) ?? []))
  const repeated = statuses.find(
    (status, index) => taken.has(status) || statuses.indexOf(status) !== index,
  )
  if (repeated !== undefined) {
    return { key: 'sites.errorPages.problems.repeated', values: { status: repeated } }
  }
  switch (form.kind) {
    case 'body':
      if (form.body.trim() === '') return { key: 'sites.errorPages.problems.bodyRequired' }
      break
    case 'file': {
      const path = form.path.trim()
      const parts = path.split('/')
      if (
        path === '' ||
        path.includes('\\') ||
        parts.some((part) => ['', '.', '..'].includes(part))
      ) {
        return { key: 'sites.errorPages.problems.pathShape' }
      }
      break
    }
    case 'redirect':
      if (form.location.trim() === '') return { key: 'sites.errorPages.problems.locationRequired' }
      return undefined
  }
  const status = optionalNumber(form.status)
  if (status !== null && (status < 200 || status > 599)) {
    return { key: 'sites.errorPages.problems.statusOverride' }
  }
  return undefined
}

/** Whether any page of `form` keeps it from being saved. */
export function pagesProblem(form: PagesForm): boolean {
  return form.pages.some((page, index) => pageProblem(page, form.pages.slice(0, index)))
}

/** The statuses pages answer, for summaries. */
export function describePages(pages?: ErrorPages | null): number[] {
  return [...new Set((pages?.pages ?? []).flatMap((page) => page.statuses))].sort((a, b) => a - b)
}

export function routePagesMode(pages?: ErrorPages | null): RoutePagesMode {
  if (pages === undefined || pages === null) return 'site'
  return (pages.pages ?? []).length > 0 ? 'own' : 'none'
}

/** A route's pages as the API stores them: `null` takes its site's. */
export function toRoutePages(mode: RoutePagesMode, form: PagesForm): ErrorPages | null {
  switch (mode) {
    case 'site':
      return null
    case 'own':
      return toPages(form)
    case 'none':
      return { pages: [], intercept: false }
  }
}

/** A site's maintenance as the form edits it; off keeps the settings. */
export interface MaintenanceForm {
  enabled: boolean
  /** One network or address per line. */
  allow: string
  status: number | string
  body: string
  contentType: string
  retryAfter: number | string
}

export function maintenanceForm(maintenance?: SiteMaintenance | null): MaintenanceForm {
  return {
    enabled: maintenance?.enabled ?? false,
    allow: (maintenance?.allow ?? []).join('\n'),
    status: maintenance?.status ?? 503,
    body: maintenance?.body ?? '',
    contentType: maintenance?.content_type ?? '',
    retryAfter: maintenance?.retry_after_seconds ?? '',
  }
}

function lines(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((line) => line.trim())
    .filter(Boolean)
}

/**
 * The maintenance as the API stores it: `null` when it was never set and is
 * off with nothing written, so a site does not gain empty settings.
 */
export function toMaintenance(
  form: MaintenanceForm,
  current?: SiteMaintenance | null,
): SiteMaintenance | null {
  const maintenance: SiteMaintenance = {
    enabled: form.enabled,
    allow: lines(form.allow),
    status: optionalNumber(form.status) ?? 503,
    body: form.body === '' ? null : form.body,
    content_type: optionalText(form.contentType),
    retry_after_seconds: optionalNumber(form.retryAfter),
  }
  const untouched =
    !form.enabled &&
    maintenance.allow?.length === 0 &&
    maintenance.status === 503 &&
    maintenance.body === null &&
    maintenance.content_type === null &&
    maintenance.retry_after_seconds === null
  return untouched && !current ? null : maintenance
}

/** Whether `value` is an IPv4 or IPv6 address, or a network of one. */
export function isNetwork(value: string): boolean {
  const [address = '', prefix, ...rest] = value.split('/')
  if (rest.length > 0 || (prefix !== undefined && !/^\d{1,3}$/.test(prefix))) return false
  const bits = prefix === undefined ? undefined : Number(prefix)
  const octets = address.split('.')
  if (octets.length === 4 && octets.every((octet) => /^\d{1,3}$/.test(octet))) {
    return octets.every((octet) => Number(octet) <= 255) && (bits === undefined || bits <= 32)
  }
  if (!address.includes(':')) return false
  try {
    new URL(`http://[${address}]/`)
  } catch {
    return false
  }
  return bits === undefined || bits <= 128
}

export function maintenanceProblem(
  form: MaintenanceForm,
): { key: string; values?: Record<string, unknown> } | undefined {
  const entries = form.allow.split('\n')
  for (const [index, line] of entries.entries()) {
    const wrong = line
      .split(',')
      .map((item) => item.trim())
      .find((item) => item !== '' && !isNetwork(item))
    if (wrong !== undefined) {
      return { key: 'sites.maintenance.problems.network', values: { line: index + 1 } }
    }
  }
  const status = optionalNumber(form.status)
  if (status !== null && (status < 200 || status > 599)) {
    return { key: 'sites.maintenance.problems.status' }
  }
  return undefined
}

export type RobotsChoice = 'routes' | Robots['kind']
export const ROBOTS_CHOICES: readonly RobotsChoice[] = [
  'routes',
  'allow_all',
  'disallow_all',
  'custom',
]

export interface RobotsForm {
  choice: RobotsChoice
  body: string
}

export function robotsForm(robots?: Robots | null): RobotsForm {
  return {
    choice: robots?.kind ?? 'routes',
    body: robots?.kind === 'custom' ? robots.body : 'User-agent: *\nDisallow:\n',
  }
}

export function toRobots(form: RobotsForm): Robots | null {
  switch (form.choice) {
    case 'routes':
      return null
    case 'custom':
      return { kind: 'custom', body: form.body }
    default:
      return { kind: form.choice }
  }
}

export function robotsProblem(form: RobotsForm): string | undefined {
  return form.choice === 'custom' && form.body.trim() === ''
    ? 'sites.robots.problems.bodyRequired'
    : undefined
}

export type FaviconChoice = 'routes' | Favicon['kind']
export const FAVICON_CHOICES: readonly FaviconChoice[] = [
  'routes',
  'no_content',
  'file',
  'redirect',
]

export interface FaviconForm {
  choice: FaviconChoice
  path: string
  location: string
}

export function faviconForm(favicon?: Favicon | null): FaviconForm {
  return {
    choice: favicon?.kind ?? 'routes',
    path: favicon?.kind === 'file' ? favicon.path : '',
    location: favicon?.kind === 'redirect' ? favicon.location : '',
  }
}

export function toFavicon(form: FaviconForm): Favicon | null {
  switch (form.choice) {
    case 'routes':
      return null
    case 'no_content':
      return { kind: 'no_content' }
    case 'file':
      return { kind: 'file', path: form.path.trim() }
    case 'redirect':
      return { kind: 'redirect', location: form.location.trim() }
  }
}

/** The gateway serves a favicon file from a directory below its static root. */
export function faviconProblem(form: FaviconForm): string | undefined {
  if (form.choice === 'file') {
    const parts = form.path.trim().split('/')
    const named = (part: string) => /^[A-Za-z0-9._-]+$/.test(part) && part !== '.' && part !== '..'
    return parts.length >= 2 && parts.every(named) ? undefined : 'sites.favicon.problems.pathShape'
  }
  if (form.choice === 'redirect') {
    const location = form.location.trim()
    const valid =
      !/\s/.test(location) &&
      (/^https?:\/\/./.test(location) || (location.startsWith('/') && !location.startsWith('//')))
    return valid ? undefined : 'sites.favicon.problems.locationShape'
  }
  return undefined
}
