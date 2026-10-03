import type {
  ApprovalPolicy,
  ApprovalPolicyInput,
  ApprovalRequest,
  ApprovalState,
  Day,
  Risk,
} from '@/api/generated'

export type Decision = 'approve' | 'reject' | 'revoke' | 'withdraw'

export const RESOURCE_KINDS = [
  'sites',
  'upstreams',
  'listeners',
  'tls-profiles',
  'security-policies',
] as const
export const DAYS: readonly Day[] = ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun']

/** Approvals that still count at `now`. */
export function validApprovals(request: ApprovalRequest, now = Date.now()): number {
  return request.approvals.filter(
    (approval) => !approval.revoked_at && Date.parse(approval.valid_until) > now,
  ).length
}

export function isOpen(request: ApprovalRequest): boolean {
  return request.state === 'pending' || request.state === 'approved'
}

export function stateVariant(
  state: ApprovalState,
): 'default' | 'secondary' | 'outline' | 'destructive' {
  switch (state) {
    case 'approved':
      return 'default'
    case 'pending':
      return 'secondary'
    case 'rejected':
      return 'destructive'
    default:
      return 'outline'
  }
}

/** What `actor` may do with `request` besides reading it. */
export function actions(
  request: ApprovalRequest,
  actor: string | undefined,
  can: (permission: string) => boolean,
): { approve: boolean; reject: boolean; revoke: boolean; withdraw: boolean } {
  const open = isOpen(request)
  const mine = request.requested_by === actor
  const approvedByMe = request.approvals.some(
    (approval) => approval.approver === actor && !approval.revoked_at,
  )
  const decides = open && !mine && can('approval.decide')
  return {
    approve: decides && !approvedByMe,
    reject: decides,
    revoke: open && approvedByMe && can('approval.decide'),
    withdraw: open && mine && can('config.apply'),
  }
}

export interface WindowForm {
  days: Day[]
  start: string
  end: string
}

export interface PolicyForm {
  id: string
  description: string
  resources: string[]
  siteTags: string
  minRisk: Risk
  windows: WindowForm[]
  approvals: number
  validMinutes: number
  enabled: boolean
}

export function policyForm(policy?: ApprovalPolicy): PolicyForm {
  return {
    id: policy?.id ?? '',
    description: policy?.description ?? '',
    resources: [...(policy?.resources ?? [])],
    siteTags: (policy?.site_tags ?? []).join(', '),
    minRisk: policy?.min_risk ?? 'low',
    windows: (policy?.windows ?? []).map((window) => ({
      days: [...(window.days ?? [])],
      start: window.start,
      end: window.end,
    })),
    approvals: policy?.approvals ?? 1,
    validMinutes: policy?.valid_minutes ?? 60,
    enabled: policy?.enabled ?? true,
  }
}

export function policyInput(form: PolicyForm): ApprovalPolicyInput {
  return {
    description: form.description.trim(),
    resources: form.resources,
    site_tags: form.siteTags
      .split(/[\s,]+/)
      .map((tag) => tag.trim())
      .filter(Boolean),
    min_risk: form.minRisk,
    windows: form.windows.map((window) => ({
      days: DAYS.filter((day) => window.days.includes(day)),
      start: window.start,
      end: window.end,
    })),
    approvals: Number(form.approvals),
    valid_minutes: Number(form.validMinutes),
    enabled: form.enabled,
  }
}
