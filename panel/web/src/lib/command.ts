/**
 * Command metadata required by every mutating management request.
 *
 * Until authenticated sessions supply the principal, the console identifies
 * itself as the calling surface; the server treats the header as metadata,
 * not as an identity claim.
 */
export const CONSOLE_ACTOR = 'web-console'
export const DEFAULT_COMMAND_TIMEOUT_MS = 30_000

// A type alias rather than an interface keeps it assignable to the generated
// client's `Record<string, unknown>` header parameter.
export type CommandHeaders = {
  'x-actor': string
  'x-deadline': string
  'Idempotency-Key': string
}

/** A fresh idempotency key; reuse it when retrying the same command. */
export function newIdempotencyKey(): string {
  return crypto.randomUUID()
}

/** Headers with an absolute RFC 3339 UTC deadline `timeoutMs` from `now`. */
export function commandHeaders(
  idempotencyKey: string,
  timeoutMs: number = DEFAULT_COMMAND_TIMEOUT_MS,
  now: number = Date.now(),
): CommandHeaders {
  return {
    'x-actor': CONSOLE_ACTOR,
    'x-deadline': new Date(now + timeoutMs).toISOString(),
    'Idempotency-Key': idempotencyKey,
  }
}
