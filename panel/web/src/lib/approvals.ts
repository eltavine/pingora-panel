import type { ApprovalRequest } from '@/api/generated'

/** Whether a reply to applying is the approval request it waits on. */
export function isApprovalRequest(value: unknown): value is ApprovalRequest {
  return (
    typeof value === 'object' && value !== null && 'requested_by' in value && 'policies' in value
  )
}
