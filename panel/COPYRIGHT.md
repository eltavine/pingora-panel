# 版权与第三方声明 / Copyright and Third-Party Notices

## 中文说明

本项目尊重开源社区及各原作者的版权和贡献。`panel` 中的原创代码及本地修改适用仓库根目录 [LICENSE](../LICENSE) 所载的 Apache License 2.0；所使用、移植或改编的第三方代码保留其原有版权声明和许可条件。项目许可证不替代第三方许可证，也不改变第三方权利人的权利。

下列第三方声明覆盖 `panel-lua/src/api/` 中改编的 Lua 模块，列出上游来源和相应许可。具体来源、适配范围及版权条款也保留在对应 `.lua` 文件头中。其他依赖的许可信息以各依赖随附的声明为准。

如您发现版权、许可或来源标注方面的问题，或有其他合理诉求，请优先联系 [me@eltavine.com](mailto:me@eltavine.com)，并尽可能提供涉及的文件、上游来源和具体问题，以便核实与修正。请广大开发者不吝指正，共同完善版权和许可信息。

## English statement

This project respects the copyrights and contributions of the open-source community and the original authors. Original code and local modifications in `panel` are covered by the Apache License 2.0 in the repository's root [LICENSE](../LICENSE). Third-party code that is used, ported or adapted retains its original copyright notices and license conditions. The project license does not replace third-party licenses or alter the rights of their copyright holders.

The third-party notices below cover the adapted Lua modules in `panel-lua/src/api/` and identify their upstream sources and licenses. Source references, the scope of adaptations and copyright terms are also retained in the corresponding `.lua` file headers. Other dependencies are subject to the notices supplied with them.

If you identify a copyright, licensing or source-attribution issue, or have another reasonable concern, please contact [me@eltavine.com](mailto:me@eltavine.com) first. Where possible, include the affected files, upstream sources and details of the concern so that it can be verified and corrected. Developers are welcome to point out errors or omissions and help improve these notices.

## 第三方来源 / Third-party sources

链接固定到具体上游版本的提交，以便核对源码和许可。文件名均相对于 `panel-lua/src/api/`。

Links refer to specific upstream revisions so that the source and license can be checked. File names are relative to `panel-lua/src/api/`.

| 上游项目 / Upstream project | 本地文件 / Local files | 许可原文 / Upstream license |
| --- | --- | --- |
| [openresty/lua-resty-core](https://github.com/openresty/lua-resty-core/tree/cb7264da6d94c259c4ad01ee5c61b5f65e3c7eb4) | [resty_core_base.lua](panel-lua/src/api/resty_core_base.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-core/blob/cb7264da6d94c259c4ad01ee5c61b5f65e3c7eb4/README.markdown#copyright-and-license) |
| [openresty/lua-resty-lrucache](https://github.com/openresty/lua-resty-lrucache/tree/3ff6300e68b73ba20e909c7d16bd839aef2e5a4b) | [lrucache.lua](panel-lua/src/api/lrucache.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-lrucache/blob/3ff6300e68b73ba20e909c7d16bd839aef2e5a4b/README.markdown#copyright-and-license) |
| [openresty/lua-tablepool](https://github.com/openresty/lua-tablepool/tree/94d3a5f43df6526fbaeb0f06873862f964766683) | [tablepool.lua](panel-lua/src/api/tablepool.lua) | [BSD-2-Clause](https://github.com/openresty/lua-tablepool/blob/94d3a5f43df6526fbaeb0f06873862f964766683/README.md#copyright-and-license) |
| [openresty/lua-resty-limit-traffic](https://github.com/openresty/lua-resty-limit-traffic/tree/ae123cf0c05e5de6d6f1445d525205ccdd74a06b) | [limit_req.lua](panel-lua/src/api/limit_req.lua), [limit_conn.lua](panel-lua/src/api/limit_conn.lua), [limit_count.lua](panel-lua/src/api/limit_count.lua), [limit_traffic.lua](panel-lua/src/api/limit_traffic.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-limit-traffic/blob/ae123cf0c05e5de6d6f1445d525205ccdd74a06b/README.md#copyright-and-license) |
| [openresty/lua-resty-redis](https://github.com/openresty/lua-resty-redis/tree/3fdc727f176f565adca8c2b350a636bd74e93de2) | [redis.lua](panel-lua/src/api/redis.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-redis/blob/3fdc727f176f565adca8c2b350a636bd74e93de2/README.markdown#copyright-and-license) |
| [openresty/lua-resty-memcached](https://github.com/openresty/lua-resty-memcached/tree/a3f161ac5ad2da6cdecd97ddbf1a9db3a7ce21a8) | [memcached.lua](panel-lua/src/api/memcached.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-memcached/blob/a3f161ac5ad2da6cdecd97ddbf1a9db3a7ce21a8/README.markdown#copyright-and-license) |
| [openresty/lua-resty-mysql](https://github.com/openresty/lua-resty-mysql/tree/64effe8ff8dd968b7621d8468d06994513576076) | [mysql.lua](panel-lua/src/api/mysql.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-mysql/blob/64effe8ff8dd968b7621d8468d06994513576076/README.markdown#copyright-and-license) |
| [openresty/lua-resty-upload](https://github.com/openresty/lua-resty-upload/tree/340830a678e94530701df9cf7a8845957d180550) | [upload.lua](panel-lua/src/api/upload.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-upload/blob/340830a678e94530701df9cf7a8845957d180550/README.markdown#copyright-and-license) |
| [openresty/lua-resty-websocket](https://github.com/openresty/lua-resty-websocket/tree/45d8e096d886c6debd0af31d3988ba90de695d72) | [websocket_client.lua](panel-lua/src/api/websocket_client.lua), [websocket_server.lua](panel-lua/src/api/websocket_server.lua), [websocket_protocol.lua](panel-lua/src/api/websocket_protocol.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-websocket/blob/45d8e096d886c6debd0af31d3988ba90de695d72/README.markdown#copyright-and-license) |
| [openresty/lua-resty-upstream-healthcheck](https://github.com/openresty/lua-resty-upstream-healthcheck/tree/f7906b6b719273bf72a0c616b050d01c6c82f818) | [healthcheck.lua](panel-lua/src/api/healthcheck.lua) | [BSD-2-Clause](https://github.com/openresty/lua-resty-upstream-healthcheck/blob/f7906b6b719273bf72a0c616b050d01c6c82f818/README.markdown#copyright-and-license) |
| [ledgetech/lua-resty-http](https://github.com/ledgetech/lua-resty-http/tree/183310324026120ab7eaf5dd82b9be90ae63aadf) | [http.lua](panel-lua/src/api/http.lua), [http_headers.lua](panel-lua/src/api/http_headers.lua) | [BSD-2-Clause](https://github.com/ledgetech/lua-resty-http/blob/183310324026120ab7eaf5dd82b9be90ae63aadf/LICENSE) |
| [nginx/nginx](https://github.com/nginx/nginx/tree/2b5c2b605b5df669da5dec6749dcc76c07d1315d) | [limit_req.lua](panel-lua/src/api/limit_req.lua), [lrucache.lua](panel-lua/src/api/lrucache.lua)（经 OpenResty 上游移植 / through OpenResty adaptations） | [BSD-2-Clause](https://github.com/nginx/nginx/blob/2b5c2b605b5df669da5dec6749dcc76c07d1315d/LICENSE) |

`limit_req.lua` 的上游注明其为 Nginx 请求限流模块的近似 Lua 移植；`lrucache.lua` 的上游注明其队列实现借用了 Nginx 的 `ngx_queue_t`。相关 Nginx 声明因此一并保留。

The upstream implementation of `limit_req.lua` identifies itself as an approximate Lua port of Nginx's request limiter. The upstream implementation of `lrucache.lua` identifies its queue implementation as borrowed from Nginx's `ngx_queue_t`. The corresponding Nginx notices are therefore retained as well.

## 版权及许可条款 / Copyright and license terms

以下保留第三方许可的英文原文。上面的中英文说明不修改这些许可条款。

The original English third-party license terms are retained below. The Chinese and English statements above do not modify these terms.

### OpenResty Lua libraries

以下版权声明分别对应所列组件；其后所载 BSD 条款适用于各组件的上游代码。

Each copyright notice below belongs to the named component. The BSD terms following these notices apply to the upstream code of each component.

**openresty/lua-resty-core**

```text
Copyright (C) 2013-2019, by Yichun "agentzh" Zhang, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-lrucache**

```text
Copyright (C) 2014-2019, by Yichun "agentzh" Zhang, OpenResty Inc.

Copyright (C) 2014-2017, by Shuxin Yang.

All rights reserved.
```

**openresty/lua-tablepool**

```text
Copyright (C) 2016-2021, by Yichun "agentzh" Zhang, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-limit-traffic**

```text
Copyright (C) 2015-2019, by Yichun "agentzh" Zhang, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-redis**

```text
Copyright (C) 2012-2017, by Yichun Zhang (agentzh) <agentzh@gmail.com>, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-memcached**

```text
Copyright (C) Yichun Zhang (agentzh), CloudFlare Inc.

Copyright (C) 2012-2017, by Yichun "agentzh" Zhang (章亦春) <agentzh@gmail.com>, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-mysql**

```text
Copyright (C) 2012-2018, by Yichun "agentzh" Zhang (章亦春) <agentzh@gmail.com>, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-upload**

```text
Copyright (C) 2012-2017, by Yichun "agentzh" Zhang (章亦春) <agentzh@gmail.com>, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-websocket**

```text
Copyright (C) 2013-2017, by Yichun Zhang (agentzh) <agentzh@gmail.com>, OpenResty Inc.

All rights reserved.
```

**openresty/lua-resty-upstream-healthcheck**

```text
Copyright (C) 2014-2017, by Yichun "agentzh" Zhang, OpenResty Inc.

All rights reserved.
```

```text
Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice, this
list of conditions and the following disclaimer in the documentation and/or
other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### lua-resty-http

```text
Copyright (c) 2013, James Hurst
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

  Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

  Redistributions in binary form must reproduce the above copyright notice, this
  list of conditions and the following disclaimer in the documentation and/or
  other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### Nginx

```text
Copyright (C) 2002-2021 Igor Sysoev
Copyright (C) 2011-2026 Nginx, Inc.
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.
```
