import { client } from '@/api/generated/client.gen'
import type { ProblemDetails } from '@/api/generated'

/**
 * Points the generated client at the management API. The console is served
 * by the same origin as the API in production, so the default is relative.
 */
export function configureApi(baseUrl: string = import.meta.env.VITE_PANEL_API_BASE_URL ?? '') {
  client.setConfig({ baseUrl })
}

/** Where the API answers a path, as the generated client joins them. */
export interface ApiLocation {
  /** The client's base URL; empty when the API shares the console's origin. */
  baseUrl: string
  /** The console page the base URL is relative to. */
  page: string
}

/** Where the API answers this page. */
export function apiLocation(): ApiLocation {
  return { baseUrl: client.getConfig().baseUrl ?? '', page: window.location.href }
}

/** `path` with the parameters of `query` that are set, where the API answers. */
export function apiUrl(path: string, query: Record<string, string>, at: ApiLocation): URL {
  const url = new URL(`${at.baseUrl}${path}`, at.page)
  for (const [name, value] of Object.entries(query)) {
    if (value) {
      url.searchParams.set(name, value)
    }
  }
  return url
}

/** `apiUrl` with the WebSocket scheme. */
export function websocketUrl(path: string, query: Record<string, string>, at: ApiLocation): string {
  const url = apiUrl(path, query, at)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  return url.toString()
}

/** A failed API call, normalized for presentation. */
export type ApiFailure =
  | { kind: 'unreachable' }
  | { kind: 'problem'; problem: ProblemDetails }
  | { kind: 'unexpected'; message: string }

function isProblemDetails(value: unknown): value is ProblemDetails {
  if (typeof value !== 'object' || value === null) {
    return false
  }
  const candidate = value as Record<string, unknown>
  return (
    typeof candidate.code === 'string' &&
    typeof candidate.title === 'string' &&
    typeof candidate.status === 'number'
  )
}

/**
 * Classifies an error thrown by the generated client: RFC 9457 problem
 * documents keep their stable code, network failures mean the API could not
 * be reached, and anything else is reported verbatim.
 */
export function toApiFailure(error: unknown): ApiFailure {
  if (isProblemDetails(error)) {
    return { kind: 'problem', problem: error }
  }
  if (error instanceof TypeError) {
    return { kind: 'unreachable' }
  }
  if (error instanceof Error) {
    return { kind: 'unexpected', message: error.message }
  }
  return { kind: 'unexpected', message: typeof error === 'string' ? error : 'Unexpected response' }
}
