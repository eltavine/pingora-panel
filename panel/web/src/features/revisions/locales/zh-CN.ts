import type { DeepString } from '@/i18n/types'

const zhCN = {
  revisions: {
    title: '配置版本',
    description: '每次应用都会记录一个版本，可比较、恢复到草稿或一键回滚。',
    emptyTitle: '还没有配置版本',
    emptyDetail: '应用草稿后，这里会记录每一次发布。',
    columns: {
      id: '版本',
      outcome: '结果',
      note: '备注',
      author: '操作人',
      created: '创建时间',
      draft: '草稿',
    },
    outcome: {
      applying: '应用中',
      active: '生效中',
      superseded: '已被取代',
      rejected: '已拒绝',
      failed: '失败',
    },
    detail: '版本 #{id}',
    settledAt: '结束时间',
    draftVersion: '草稿版本',
    languageVersion: '语言版本',
    gatewayRevision: '网关版本',
    contentHash: '内容哈希',
    snapshotHash: '快照哈希',
    diagnostics: '未生效原因',
    noNote: '无备注',
    editNote: '编辑备注',
    noteSaved: '备注已保存',
    tabs: {
      changes: '变更',
      files: '文件',
    },
    against: '对比',
    againstPrevious: '上一版本',
    againstActive: '生效版本',
    againstDraft: '当前草稿',
    identical: '两者完全一致',
    restore: '恢复到草稿',
    restoreTitle: '将版本 #{id} 恢复到草稿？',
    restoreDetail: '草稿会替换为该版本的文件，应用后才会生效。',
    restored: '已将版本 #{id} 恢复到草稿',
    rollback: '回滚到此版本',
    rollbackTitle: '回滚到版本 #{id}？',
    rollbackDetail: '会将该版本恢复到草稿并立即应用，生成一个新版本。',
    rollbackReason: '原因',
    rollbackNote: '回滚到版本 #{id}',
    rolledBack: '已回滚，版本 #{revision} 生效',
  },
}

/** Messages of the revision pages; each language defines all of them. */
export type RevisionsMessages = DeepString<typeof zhCN>

export default zhCN
