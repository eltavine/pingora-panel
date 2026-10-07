import type { DeepString } from '@/i18n/types'

const zhCN = {
  httpPolicies: {
    title: 'HTTP 策略',
    description: '按网站或路由修改请求与响应字段、Server 字段，允许跨源请求并压缩响应。',
    listTitle: '策略',
    listHint:
      '网站的策略作用于它的全部请求；路由的策略在网站之后生效，其 CORS 与压缩会取代网站的设置。',
    new: '新建策略',
    edit: '编辑策略',
    id: '标识',
    idHint: '小写字母、数字和连字符。',
    idTaken: '此标识已被使用',
    does: '作用',
    usedBy: '使用者',
    unused: '未使用',
    nothing: '无',
    inUse: '仍有网站或路由使用此策略',
    saved: '已保存 HTTP 策略 {id}',
    deleted: '已删除 HTTP 策略',
    confirmDeleteTitle: '删除 HTTP 策略 {id}？',
    confirmDeleteDetail: '只能删除没有网站或路由使用的策略。',
    emptyTitle: '还没有 HTTP 策略',
    emptyDetail:
      '策略可以设置、添加和删除请求与响应字段，删除或替换 Server 字段，允许跨源请求并压缩响应。',
    sections: {
      request: '请求字段',
      response: '响应字段',
      server: 'Server 字段',
      cors: '跨源请求',
      compression: '压缩',
    },
    requestHint:
      '在请求发往上游之前修改。值可以使用 $host、$uri、$method、$scheme、$client_ip、$request_id、$http_<name> 和 $cookie_<name>；Host、分帧与转发字段保持网关写入的值。',
    responseHint:
      '代理的响应和网关生成的响应都会修改；分帧字段与 Content-Encoding 保持网关写入的值。',
    operation: '操作',
    operations: {
      set: '设置',
      add: '添加',
      remove: '删除',
    },
    fieldName: '字段',
    fieldNameInvalid: '字段名形如 X-Frame-Options，不含空格',
    fieldValue: '值',
    addChange: '添加字段修改',
    removeChange: '移除字段修改',
    server: '处理方式',
    serverModes: {
      keep: '保留上游的值',
      remove: '删除',
      replace: '替换',
    },
    serverValue: 'Server 值',
    cors: '允许跨源请求',
    corsHint: '网关自行应答预检请求，并允许列出的来源读取响应（Fetch 标准的 CORS 协议）。',
    origins: '允许的来源',
    originsHint: '每行一个，如 https://shop.example、https://*.shop.example 或 *。',
    methods: '允许的方法',
    methodsHint: 'GET、HEAD 和 POST 之外的方法，以空格分隔，如 PUT DELETE。',
    headers: '允许的请求字段',
    headersHint: '以空格分隔，如 X-Api-Key。',
    expose: '暴露的响应字段',
    exposeHint: '脚本可以读取的字段，如 X-Request-Id。',
    credentials: '允许携带凭据',
    credentialsHint: '请求会带上 Cookie 与认证信息；来源会被原样回显，因此不能与 * 同用。',
    maxAge: '预检缓存（秒）',
    maxAgeHint: '最多 86400 秒；留空由浏览器决定。',
    compression: '压缩响应',
    compressionHint: '使用客户端接受的编码；已编码、部分、HEAD 和 no-transform 的响应原样发送。',
    codings: '编码',
    codingsHint: '客户端通过 Accept-Encoding 从中选择。',
    types: '媒体类型',
    typesHint: '每行一个，如 text/html 或 text/*。',
    minSize: '最小大小',
    minSizeHint: '更小的响应原样发送；留空则不限大小。',
    invalidSize: '请填写大小，如 512、1k 或 1m',
    effects: {
      request: '请求字段',
      response: '响应字段',
      server: 'Server',
      cors: 'CORS',
      compression: '压缩',
    },
  },
}

/** Messages of the HTTP policy pages; each language defines all of them. */
export type HttpPoliciesMessages = DeepString<typeof zhCN>

export default zhCN
