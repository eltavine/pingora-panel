import { client } from '@/api/generated/client.gen'
import type { ProblemDetails } from '@/api/generated'

/**
 * Points the generated client at the management API. The console is served
 * by the same origin as the API in production, so the default is relative.
 */
export function configureApi(baseUrl: string = import.meta.env.VITE_PANEL_API_BASE_URL ?? '') {
  client.setConfig({ baseUrl })
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
