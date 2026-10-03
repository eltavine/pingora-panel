import type { RevisionOutcome } from '@/api/generated'
import type { StatusTone } from '@/components/StatusIndicator.vue'

/** How each outcome reads: the running one is positive, failures negative. */
export const outcomeTones: Record<RevisionOutcome, StatusTone> = {
  applying: 'pending',
  active: 'positive',
  superseded: 'neutral',
  rejected: 'warning',
  failed: 'negative',
}

/** What a revision can be compared with. */
export type Comparison = 'previous' | 'active' | 'draft' | `${number}`

export function isComparison(value: unknown): value is Comparison {
  return (
    typeof value === 'string' &&
    (['previous', 'active', 'draft'].includes(value) || /^\d+$/.test(value))
  )
}
