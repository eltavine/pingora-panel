import { useQueryClient } from '@tanstack/vue-query'
import { toast } from 'vue-sonner'
import { toApiFailure } from './api'
import { commandHeaders, newIdempotencyKey, type CommandHeaders } from './command'

/** Headers for one change to the representation tagged `etag` (RFC 9110 If-Match). */
export function changeHeaders(etag: string): CommandHeaders & { 'If-Match': string } {
  return { ...commandHeaders(newIdempotencyKey()), 'If-Match': etag }
}

/** Headers for a change that needs no precondition. */
export function plainHeaders(): CommandHeaders {
  return commandHeaders(newIdempotencyKey())
}

/** Any configuration change can affect every configuration view. */
export function useRefreshConfiguration() {
  const client = useQueryClient()
  return () => client.invalidateQueries()
}

/** A failed change as a toast, with diagnostics when the API sent them. */
export function notifyFailure(error: unknown, title: string) {
  const failure = toApiFailure(error)
  const description =
    failure.kind === 'problem'
      ? [
          failure.problem.detail,
          ...(failure.problem.field_errors ?? []).map((item) => item.message),
        ]
          .filter(Boolean)
          .join('\n')
      : failure.kind === 'unexpected'
        ? failure.message
        : undefined
  toast.error(title, { description })
}
