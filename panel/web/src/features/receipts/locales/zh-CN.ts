import type { DeepString } from '@/i18n/types'

const zhCN = {
  receipts: {
    title: '发布回执',
    description: '按幂等键查询激活请求的最终结果。',
    key: '幂等键',
    lookup: '查询',
    pending: '处理中',
    succeeded: '已成功',
    rejected: '已拒绝',
    failedBeforeCommit: '提交前失败',
    pendingReconciliation: '等待对账',
    unknown: '结果未知',
    emptyTitle: '输入幂等键',
    emptyDetail: '每次激活请求都会记录一张回执，可用于重试或审计。',
  },
}

/** Messages of the receipt pages; each language defines all of them. */
export type ReceiptsMessages = DeepString<typeof zhCN>

export default zhCN
