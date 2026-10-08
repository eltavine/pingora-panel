import type { DeepString } from '@/i18n/types'

const zhCN = {
  system: {
    title: '系统',
    description: '正在运行的各部分版本、能否开始升级，以及去除机密后的诊断包。',
    versions: {
      title: '版本',
      description: '发行版、各模块、网关、主机代理与已部署的镜像。',
      release: '发行版',
      api: 'API',
      language: '配置语言',
      irSchema: 'IR 架构',
      gateway: '网关',
      gatewayVersions: '{gateway} · 引擎 {engine} · 适配器 {adapter}',
      agent: '主机代理',
      agentVersions: '{build} · 协议 {protocol} · {hostname}',
      unavailable: '未连接',
      deployment: '部署',
      deployed: '{action}，{engine}，{time}',
      previous: '之前为 {release}',
      notDeployed: '未经生命周期工具部署',
      modules: '模块',
      module: '模块',
      build: '构建',
      schema: '数据库架构',
      protocols: '协议修订',
      protocol: '{name} {min}–{max}',
      noModules: '服务目录里还没有模块',
      images: '已部署的镜像',
      service: '服务',
      image: '镜像',
      digest: '摘要',
      localBuild: '本地构建',
      problems: '有些内容未能读取',
    },
    actions: {
      install: '安装',
      upgrade: '升级',
      rollback: '回滚',
      restore: '恢复',
    },
    readiness: {
      title: '升级就绪',
      description:
        '各模块是否健康，网关上是否有已准备或正在应用的内容，最近一次备份的时间，以及再做一次备份的空间。',
      ready: '可以开始升级',
      notReady: '暂时不能升级',
      hint: '主机上的 pingora-panel upgrade 会先做同样的检查，再备份全部数据。',
      checks: {
        modules: '模块',
        gateway: '网关',
        backup: '备份',
        space: '磁盘空间',
      },
      states: {
        pass: '通过',
        warn: '注意',
        fail: '未通过',
      },
    },
    diagnostics: {
      title: '诊断包',
      description:
        '一个 JSON 文件：版本、就绪情况、健康状态、网关与主机、配置数量、最近的失败与审计事件。机密在离开面板前已被移除。',
      download: '下载诊断包',
      downloaded: '已下载 {name}',
      failed: '无法生成诊断包',
      needs: '下载诊断包需要 platform.diagnose 权限。',
      withheld: '你无权查看的部分会被省略并在文件中注明。',
    },
  },
}

/** Messages of the system page; each language defines all of them. */
export type SystemMessages = DeepString<typeof zhCN>

export default zhCN
