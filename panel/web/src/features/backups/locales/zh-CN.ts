import type { DeepString } from '@/i18n/types'

const zhCN = {
  backups: {
    title: '备份',
    description: '数据库与网站目录的归档，按清单校验后原位恢复；主密钥从不包含在内。',
    refresh: '刷新',
    take: '创建备份',
    takeTitle: '创建备份',
    takeDetail: '备份在后台进行并保存在控制面的数据目录中；新的备份完成后会移除较旧的备份。',
    takingNow: '正在创建备份',
    takeFailed: '未能开始备份',
    taken: '时间',
    by: '由 {actor}',
    holds: '内容',
    state: '状态',
    size: '大小',
    actionsFor: '{time} 的备份的操作',
    download: '下载归档',
    restore: '恢复',
    restoreSites: '恢复网站目录…',
    restoreSitesTitle: '恢复网站目录',
    restoreSitesDetail:
      '用备份中的副本替换网站目录下的该目录；网关只会提供旧文件或新文件，不会混合。',
    sitesRestored: '已恢复 {path}：{count} 个文件',
    restoreConfiguration: '恢复配置…',
    restoreConfigurationTitle: '恢复配置',
    restoreConfigurationDetail:
      '把备份时网关运行的配置保存为草稿并替换当前草稿，之后像其他修改一样审阅并应用。',
    configurationRestored: '已保存为草稿 v{version}',
    review: '审阅',
    restoreFailed: '未恢复任何内容',
    remove: '删除',
    removeTitle: '删除这个备份？',
    removeDetail: '其归档将被删除，且无法撤销。',
    removed: '已删除备份',
    removeFailed: '未能删除备份',
    empty: '还没有备份',
    emptyDetail: '创建一个备份以保存配置、证书、数据库或网站。',
    siteDirectory: '网站目录下的目录',
    allSites: '留空则为全部网站',
    contents: {
      configuration: '配置',
      certificates: '证书',
      databases: '数据库',
      sites: '网站',
    },
    explained: {
      configuration: '配置数据库，并以配置包附带草稿与当前生效的修订。',
      certificates: '密钥已封装的证书、ACME 账户与 DNS 服务商。',
      databases: '所有模块的数据库，包括审计记录与账户。',
      sites: '网关提供的文件，或其中一个目录。',
    },
    states: {
      pending: '等待中',
      running: '进行中',
      completed: '已完成',
      failed: '失败',
    },
  },
}

/** Messages of the backup pages; each language defines all of them. */
export type BackupsMessages = DeepString<typeof zhCN>

export default zhCN
