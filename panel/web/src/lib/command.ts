/**
 * Command metadata required by every mutating management request. The actor
 * is the logged-in account, which the API knows from the session.
 */
export const DEFAULT_COMMAND_TIMEOUT_MS = 30_000

// A type alias rather than an interface keeps it assignable to the generated
// client's `Record<string, unknown>` header parameter.
export type CommandHeaders = {
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
    'x-deadline': new Date(now + timeoutMs).toISOString(),
    'Idempotency-Key': idempotencyKey,
  }
}
