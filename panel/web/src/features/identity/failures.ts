import type { Composer } from 'vue-i18n'
import { toApiFailure } from '@/lib/api'

/** What a failed login or setup tells the person at the keyboard. */
export function signInProblem(error: unknown, t: Composer['t']): string {
  const failure = toApiFailure(error)
  switch (failure.kind) {
    case 'unreachable':
      return t('auth.unreachable')
    case 'unexpected':
      return failure.message
    case 'problem':
      switch (failure.problem.status) {
        case 401:
          return t('auth.invalid')
        case 429:
          return t('auth.throttled')
        case 403:
          return failure.problem.code === 'PERMISSION_DENIED' &&
            failure.problem.detail?.includes('locked')
            ? t('auth.locked')
            : (failure.problem.detail ?? t('auth.failed'))
        default:
          return failure.problem.detail ?? t('auth.failed')
      }
  }
}

/** Problems the API reported for one field, such as `password`. */
export function fieldProblems(error: unknown, field: string): string[] {
  const failure = toApiFailure(error)
  if (failure.kind !== 'problem') {
    return []
  }
  return (failure.problem.field_errors ?? [])
    .filter((item) => item.resource_id === field)
    .flatMap((item) => [item.message, item.help].filter((text): text is string => !!text))
}
