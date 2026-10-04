import { useQueryClient } from '@tanstack/vue-query'
import { toast } from 'vue-sonner'
import { toApiFailure } from './api'
import { commandHeaders, newIdempotencyKey, type CommandHeaders } from './command'
import { invalidateTagged } from './query'

/** Headers for one change to the representation tagged `etag` (RFC 9110 If-Match). */
export function changeHeaders(etag: string): CommandHeaders & { 'If-Match': string } {
  return { ...commandHeaders(newIdempotencyKey()), 'If-Match': etag }
}

/** Headers for a change that needs no precondition. */
export function plainHeaders(): CommandHeaders {
  return commandHeaders(newIdempotencyKey())
}

/**
 * What a change of the draft can affect: the configuration's views, and
 * approvals, whose state follows the draft's content.
 */
const DRAFT_TAGS = ['configuration', 'approvals'] as const

/** Applying the draft also changes what the gateway runs. */
const APPLIED_TAGS = [...DRAFT_TAGS, 'gateway'] as const

/** Refreshes what a change of the draft can affect. */
export function useRefreshConfiguration() {
  const client = useQueryClient()
  return () => invalidateTagged(client, DRAFT_TAGS)
}

/** Refreshes what applying the draft can affect. */
export function useRefreshApplied() {
  const client = useQueryClient()
  return () => invalidateTagged(client, APPLIED_TAGS)
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
