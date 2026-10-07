import type { DeepString } from '@/i18n/types'

const zhCN = {
  siteFiles: {
    title: '网站文件',
    description: '网关提供静态网站的目录下的文件。',
    location: '位置',
    root: '网站',
    name: '名称',
    size: '大小',
    modified: '修改时间',
    upload: '上传',
    uploaded: '已上传 {count} 个文件',
    uploadFailed: '无法上传 {name}',
    newFolder: '新建文件夹',
    folderName: '文件夹名称',
    folderCreated: '已创建 {name}',
    edit: '编辑 {name}',
    view: '查看 {name}',
    download: '下载 {name}',
    downloadFailed: '无法下载该文件',
    remove: '删除 {name}',
    removeTitle: '删除 {name}？',
    removeDetail: '删除后无法恢复，网关会立即停止提供它。',
    recursive: '连同其中的全部内容',
    removed: '已删除 {name}',
    empty: '此文件夹为空',
    emptyDetail: '在这里上传文件，或把文件拖到此卡片上。',
    content: '{path} 的内容',
    editDetail: '保存会立即替换该文件，网关随即提供新内容。',
    viewDetail: '只能查看：修改文件需要 files.write 权限。',
    saved: '已保存 {path}',
    stale: '打开之后它已被修改；请重新加载查看现在的内容。',
    reload: '重新加载',
  },
}

/** Messages of the site file pages; each language defines all of them. */
export type SiteFilesMessages = DeepString<typeof zhCN>

export default zhCN
