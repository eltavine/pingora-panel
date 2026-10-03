# 0019: Approving configuration changes

Status: accepted.

## Context

Teams want a second person to look at risky changes before the gateway runs
them, without slowing down routine ones, and they need a way past the rule
when production is down and nobody else is awake. Applying the draft
([ADR 0011](0011-configuration-model-and-apply.md),
[ADR 0012](0012-configuration-language-and-revisions.md)) validates the
configuration, records a revision and publishes it in one step; approvals
have to fit in front of publishing without making the draft or the
revision history depend on them.

## Decision

- **Policies.** Approval policies belong to `config-service`, beside the
  draft but not inside it, so changing a policy never needs an approval of
  its own. A policy names the changes it covers: changed resources of given
  kinds (`sites`, `listeners`, `tls-profiles` and so on), sites carrying
  given tags (the environment labels), a minimum risk, and time windows in
  UTC. Every condition a policy sets must hold; one it leaves empty holds
  always. A policy also says how many people must approve and for how long
  an approval stays valid. Policies have a version that every change
  increases. Managing them needs `approval.manage`.
- **Risk.** A change is high-risk when it removes anything or touches
  listeners, TLS profiles or security policies, and low-risk otherwise. The
  rule is fixed and computed from the same plan people review.
- **Requests.** Applying a draft that some enabled policy covers opens an
  approval request instead of publishing, or returns the open one for the
  same content. The request pins the draft's content hash, the plan, the
  covering policies with their versions, the number of approvals needed and
  who asked. Applying again goes ahead once enough approvals are valid; the
  request is then closed as applied.
- **Independence.** Approving and rejecting need `approval.decide`, and
  nobody may decide on their own request. Each person approves at most once;
  their approval stays valid for the shortest validity of the covering
  policies and can be revoked until the request is applied. The requester
  can withdraw it. Requests nobody decides on expire after a day.
- **Invalidation.** An approval only stands for what was approved: when the
  draft's content differs, when a covering policy changed or when the
  approvals have expired, applying opens a new request rather than using
  the old one.
- **Emergency bypass.** People with `approval.bypass`, which only
  Administrators hold, may apply without the approvals by giving a reason
  and an incident reference. The bypass is recorded as its own audit event,
  `config.approval.bypassed`, in the same transaction as the revision it
  lets through, and the console shows recent bypasses on the approvals
  page.
- **Events.** Policy changes and every step of a request are published like
  other configuration events, so the audit trail shows who asked, who
  approved or rejected, and what was finally applied.

## Consequences

- Publishing snapshots directly to the gateway (`gateway.publish`) stays
  outside approvals; it is a lower-level operation for Administrators and
  recovery, and is audited as before.
- Site groups (IAM-026) can later become another condition without changing
  how requests and approvals work.
- An approval is evidence about one exact configuration; editing anything
  afterwards means asking again.
