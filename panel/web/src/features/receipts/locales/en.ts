import type { ReceiptsMessages } from './zh-CN'

const en: ReceiptsMessages = {
  receipts: {
    title: 'Receipts',
    description: 'Look up the final outcome of an activation by its idempotency key.',
    key: 'Idempotency key',
    lookup: 'Look up',
    pending: 'In progress',
    succeeded: 'Succeeded',
    rejected: 'Rejected',
    failedBeforeCommit: 'Failed before commit',
    pendingReconciliation: 'Awaiting reconciliation',
    unknown: 'Outcome unknown',
    emptyTitle: 'Enter an idempotency key',
    emptyDetail: 'Every activation request records a receipt for retries and audits.',
  },
}

export default en
