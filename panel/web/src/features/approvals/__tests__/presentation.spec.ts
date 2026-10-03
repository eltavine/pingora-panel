import { describe, expect, it } from 'vitest'
import type { ApprovalRequest } from '@/api/generated'
import { isApprovalRequest } from '@/lib/approvals'
import { actions, policyForm, policyInput, validApprovals } from '../presentation'

const request: ApprovalRequest = {
  id: 'r1',
  state: 'pending',
  draft_version: 3,
  content_hash: 'sha256:aa',
  requested_by: 'alice',
  requested_at: '2026-10-03T09:00:00Z',
  expires_at: '2026-10-04T09:00:00Z',
  risk: 'high',
  policies: [{ id: 'prod', version: 1 }],
  required: 1,
  valid_minutes: 60,
  changes: [],
  approvals: [],
}

describe('approval actions', () => {
  const everything = () => true

  it('let others decide and the requester withdraw', () => {
    expect(actions(request, 'bob', everything)).toEqual({
      approve: true,
      reject: true,
      revoke: false,
      withdraw: false,
    })
    expect(actions(request, 'alice', everything)).toEqual({
      approve: false,
      reject: false,
      revoke: false,
      withdraw: true,
    })
  })

  it('let an approver revoke, and nobody act on closed requests', () => {
    const approved: ApprovalRequest = {
      ...request,
      state: 'approved',
      approvals: [
        {
          approver: 'bob',
          approved_at: '2026-10-03T09:05:00Z',
          valid_until: '2099-01-01T00:00:00Z',
          revoked_at: null,
        },
      ],
    }
    expect(actions(approved, 'bob', everything).revoke).toBe(true)
    expect(actions(approved, 'bob', everything).approve).toBe(false)
    expect(validApprovals(approved)).toBe(1)
    const closed = { ...approved, state: 'applied' as const }
    expect(Object.values(actions(closed, 'bob', everything)).some(Boolean)).toBe(false)
  })

  it('need the permissions', () => {
    expect(actions(request, 'bob', () => false).approve).toBe(false)
  })
})

describe('approval policy form', () => {
  it('round-trips a policy and orders window days', () => {
    const form = policyForm()
    form.id = 'prod'
    form.siteTags = 'prod, payments  eu'
    form.windows = [{ days: ['fri', 'mon'], start: '09:00', end: '18:00', timeZone: 'UTC' }]
    expect(policyInput(form)).toEqual({
      description: '',
      resources: [],
      site_tags: ['prod', 'payments', 'eu'],
      min_risk: 'low',
      windows: [
        {
          recurrence: expect.stringMatching(
            /^DTSTART:\d{8}T090000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO,FR$/,
          ),
          minutes: 540,
        },
      ],
      approvals: 1,
      valid_minutes: 60,
      enabled: true,
    })
  })
})

describe('isApprovalRequest', () => {
  it('tells a waiting change from an applied revision', () => {
    expect(isApprovalRequest(request)).toBe(true)
    expect(isApprovalRequest({ revision: 4, draft: {} })).toBe(false)
  })
})
