<p align="center">
  <img src="panel/web/public/favicon.svg" alt="Pingora Panel 标志" width="112">
</p>

<h1 align="center">Pingora Panel</h1>

<p align="center"><strong>自托管的网站网关控制平台，基于 Pingora：变更经过评审，原子应用，全程审计。</strong></p>

<p align="center">
  <strong>
    <a href="#快速开始">安装</a> ·
    <a href="panel/README.md">文档</a> ·
    <a href="PRODUCT_SPEC.md">产品规格</a>
  </strong>
</p>

<p align="center">
  <a href="https://github.com/eltavine/pingora-panel/actions/workflows/panel.yml"><img src="https://img.shields.io/github/actions/workflow/status/eltavine/pingora-panel/panel.yml?branch=main&amp;style=flat-square&amp;label=build&amp;logo=githubactions&amp;logoColor=white" alt="构建工作流状态"></a>
  <a href="https://github.com/eltavine/pingora-panel/actions/workflows/panel-deploy.yml"><img src="https://img.shields.io/github/actions/workflow/status/eltavine/pingora-panel/panel-deploy.yml?branch=main&amp;style=flat-square&amp;label=deploy&amp;logo=docker&amp;logoColor=white" alt="部署检查状态"></a>
  <a href="https://github.com/eltavine/pingora-panel/actions/workflows/audit.yml"><img src="https://img.shields.io/github/actions/workflow/status/eltavine/pingora-panel/audit.yml?branch=main&amp;style=flat-square&amp;label=audit&amp;logo=rust&amp;logoColor=white" alt="安全审计状态"></a>
  <a href="#兼容性"><img src="https://img.shields.io/badge/Rust-1.94%2B-000000?style=flat-square&amp;logo=rust&amp;logoColor=white" alt="Rust 1.94 或更高版本"></a>
  <a href="https://github.com/cloudflare/pingora"><img src="https://img.shields.io/badge/Pingora-0.9.0-F38020?style=flat-square&amp;logo=cloudflare&amp;logoColor=white" alt="Pingora 0.9.0"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/eltavine/pingora-panel?style=flat-square" alt="许可证"></a>
</p>

<p align="center">
  <a href="README.md">English</a> &nbsp; <a href="README_ZH.md">简体中文</a>
</p>

# 概述

Pingora Panel 在一台主机上运行基于 Cloudflare [Pingora](https://github.com/cloudflare/pingora) 的网关，并供团队共同管理。网站及其域名、路由、上游、监听、证书、安全策略与 HTTP 策略都先在草稿中编辑，经过检查，在审批策略要求时由他人审批，再原子地应用到网关。每次应用都保存为一个版本，可以对比，也可以回滚。

Web 控制台、`ppanel` 命令行和你自己的自动化都使用同一套 REST API。每个请求都经过认证与授权，每次变更和被拒绝的尝试都记录在防篡改的审计日志中，网关的流量、日志与告警和它的配置放在一起查看。草稿同时也是一组 Nginx 风格的配置文件，可以检查、格式化、比较差异，并纳入版本控制。

# 快速开始

1. **准备一台 Linux 主机**，安装 Docker Engine 与 Compose v2，或带 Compose 的 Podman，然后克隆本仓库。安装过程从源码构建镜像，目前还没有发布的镜像或版本。
2. **生成密钥**到 `panel/deploy/secrets/`（不会提交到仓库）：一次性引导令牌、密码 pepper，以及封存证书私钥的主密钥。

   ```bash
   panel/deploy/generate-secrets.sh
   ```

   请把主密钥和 `control-data` 卷一起备份；没有它，已保存的私钥无法打开。
3. **构建并启动**。所有容器使用主机网络并绑定回环地址；网关按监听的配置对外监听。

   ```bash
   docker compose -f panel/deploy/compose.yaml up -d --build
   ```

4. **打开控制台** <http://127.0.0.1:8080>；从其他机器访问时，使用 `ssh -L 8080:127.0.0.1:8080 <host>` 建立隧道。首次访问会要求输入 `panel/deploy/secrets/bootstrap-token` 中的引导令牌，并创建第一个管理员。
5. **发布一个网站**：在控制台中操作，或在控制台为你的账号创建 API 令牌后使用命令行：

   ```bash
   export PPANEL_TOKEN=ppat_...
   ppanel() { docker compose -f panel/deploy/compose.yaml exec -T -e PPANEL_TOKEN control ppanel "$@"; }
   ppanel listener set public --address 0.0.0.0:80
   ppanel upstream create --name app --node 10.0.0.11:8080
   ppanel site create --name shop --domain shop.example --proxy <upstream-id>
   ppanel config apply
   ```

如需同时管理主机及其容器，先用 `panel/deploy/ops-agent/install.sh` 安装主机代理，再在 Compose 命令中加上 `-f panel/deploy/compose.ops-agent.yaml`。

## 一次变更如何到达网关

| 步骤 | 发生什么 |
| :--- | :--- |
| **编辑** | 变更进入草稿。会让配置失效的变更整体被拒绝。 |
| **检查** | `ppanel config check` 与控制台在问题所在的位置给出每个问题；`ppanel config plan` 按资源列出应用后会改变什么。 |
| **审批** | 审批策略覆盖的变更要等其他人批准。 |
| **应用** | 草稿编译为快照，网关先准备，再原子地激活，且只在期间没有其他应用时生效。网关拒绝的快照不会改变任何东西。 |
| **回滚** | 每次应用都是一个版本，可以对比、备注，并用 `ppanel config rollback --to <revision>` 恢复。 |

# 功能覆盖

当前的功能领域，每项都链接到说明它做什么、为什么这样做以及不做什么的决策记录：

[`网关`](./docs/adr/0010-pingora-data-plane.md) · [`网站与应用`](./docs/adr/0011-configuration-model-and-apply.md) · [`配置语言`](./docs/adr/0012-configuration-language-and-revisions.md) · [`路由条件`](./docs/adr/0036-route-conditions.md) · [`HTTP 策略`](./docs/adr/0037-http-policies.md) · [`改写与内部重定向`](./docs/adr/0040-rewrites-and-internal-redirects.md) · [`错误页与维护`](./docs/adr/0041-error-pages-and-maintenance.md) · [`静态内容`](./docs/adr/0042-directory-listings-media-types-and-cache-headers.md) · [`代理缓存`](./docs/adr/0043-proxy-cache.md) · [`插件`](./docs/adr/0044-external-plugins-and-provider-ports.md) · [`上游韧性`](./docs/adr/0038-upstream-resilience-and-streams.md) · [`安全策略`](./docs/adr/0017-request-security-policies.md) · [`证书`](./docs/adr/0015-certificates-and-secret-material.md) · [`ACME`](./docs/adr/0016-acme-issuance-and-renewal.md) · [`账号与权限`](./docs/adr/0014-identity-and-access.md) · [`单点登录`](./docs/adr/0018-identity-provider-sign-in.md) · [`服务账号`](./docs/adr/0020-service-accounts-and-workload-identity.md) · [`限定授权`](./docs/adr/0021-scoped-and-conditional-grants.md) · [`审批`](./docs/adr/0019-change-approvals.md) · [`审计日志`](./docs/adr/0013-audit-trail.md) · [`指标、日志与追踪`](./docs/adr/0022-metrics-logs-and-traces.md) · [`访问日志`](./docs/adr/0025-access-and-error-logs.md) · [`日志搜索`](./docs/adr/0026-log-search-tail-and-deletion.md) · [`告警`](./docs/adr/0027-alerts.md) · [`主机与容器`](./docs/adr/0028-host-and-container-operations.md) · [`主机代理`](./docs/adr/0030-ops-agent.md) · [`容器`](./docs/adr/0031-containers.md) · [`容器网站`](./docs/adr/0033-sites-in-front-of-containers.md) · [`站点文件`](./docs/adr/0034-site-files.md) · [`备份`](./docs/adr/0035-backups.md) · [`基准测试`](./docs/adr/0024-gateway-benchmarks.md)

[`panel/README.md`](./panel/README.md) 说明每个领域的 API、命令行与配置语言，[`PRODUCT_SPEC.md`](./PRODUCT_SPEC.md) 列出每项已编目功能及其状态。

# 工作原理

- **网关：** `gatewayd` 运行 Pingora 0.9，并通过适配器把与引擎无关的配置快照转换为监听、虚拟主机、路由、TLS、静态内容和上游池。快照先准备、再原子激活；最近一次可用的快照保存在磁盘上，重启后继续使用，重载和调整 worker 不会中断连接。
- **契约：** 控制面通过 gRPC 访问网关，使用 proto3 契约和双向 TLS，证书由内部 CA 签发并自动续期。快照声明它需要的能力，网关不认识的设置会被拒绝，而不是被忽略。
- **控制面：** `panel-control` 在一个进程中运行 API、配置、自动化、可观测与审计模块。每个模块拥有自己的 SQLite 数据库，并通过事务 outbox 把 CloudEvents 发布到 NATS JetStream。
- **同一套 API：** REST API 由 OpenAPI 描述，每次提交都检查是否有破坏性变更；控制台的客户端由它生成，`ppanel` 覆盖同样的操作。
- **配置即文本：** 草稿是以 `main.conf` 为入口的 Nginx 风格配置语言，支持检查、格式化、补全，以及应用前的变更计划。已有的 Nginx 配置可以转换过来。
- **可观测：** 网关暴露 Prometheus 指标并写结构化访问日志，由安装中的采集器送往 Loki；W3C Trace Context 透传到上游，其追踪 ID 随每个请求记录。控制台绘制流量图表、搜索和实时查看日志，告警通过签名的 Webhook 通知。

# 兼容性

| 领域 | 说明 |
| :--- | :--- |
| **主机** | 安装 Docker Engine 与 Compose v2 或 Podman 的 Linux。镜像与运行中的安装在 CI 中于 Linux x86_64 上构建并检查。 |
| **协议** | HTTP/1.1 与 HTTP/2（TLS 上经 ALPN 协商，明文监听上为 h2c），支持 WebSocket 升级、Server-Sent Events 与 gRPC。HTTP/3 已预留，目前会被拒绝。 |
| **TLS** | 基于 rustls，支持 TLS 1.2 与 1.3、按 SNI 为每个主机选择证书，以及 HSTS。证书可以上传，也可以通过 ACME（RFC 8555）用 HTTP-01，或经 RFC 2136 用 DNS-01 签发，支持外部账号绑定。 |
| **控制台** | 当前的桌面与移动浏览器；端到端测试在 Chromium 与 WebKit 中运行。支持英文与简体中文，以及浅色与深色主题。 |
| **工具链** | Rust 1.94 或更高版本；控制台需要 Node.js 22.18 或更高（或 24.12 或更高）及 pnpm 11。 |
| **Pingora** | 0.9.0，随本仓库一起维护。定时任务会用 Pingora 的 main 分支构建适配器，以便提前发现上游变化。 |

# 架构

```text
pingora-panel/
├─ pingora-*/, tinyufo/   # Pingora 0.9.0 的各个 crate，本地改动均有记录
├─ panel/                 # Pingora Panel 工作区
│  ├─ proto/              # gRPC 与事件契约，proto3，由 Buf 检查
│  ├─ panel-domain/       # 领域值；与 panel-ir、panel-errors 一起构成稳定核心
│  ├─ panel-engine/       # 网关端口、IR 校验与内存引擎
│  ├─ gateway-pingora/    # Pingora 数据面适配器
│  ├─ gatewayd/           # 网关进程
│  ├─ panel-control/      # 控制面进程及其 *-service 模块
│  ├─ panel-api/          # REST API 及其 OpenAPI 文档
│  ├─ panel-config-*/     # 配置模型、配置语言与编解码
│  ├─ panel-cli/          # ppanel 命令行
│  ├─ ops-agent/          # 可选的主机代理
│  ├─ web/                # 控制台：Vue、shadcn-vue 与生成的 API 客户端
│  └─ deploy/             # 容器镜像与 Compose 安装
├─ docs/adr/              # 架构决策记录
├─ docs/                  # Pingora 自带的指南、网关运维手册与本地补丁记录
├─ .github/               # 工作流、策略与仓库守卫
└─ PRODUCT_SPEC.md        # 产品范围、功能目录、路线图与质量门禁
```

`GatewayEngine`、`DataPlaneAdapter`、`SnapshotStore` 等端口位于核心 crate 中。Pingora、存储、gRPC 与 HTTP 都在叶子适配器 crate 里，`gatewayd` 与 `panel-control` 只负责组装；生成的 protobuf 类型和 Pingora 类型不会越过稳定端口。依赖方向由 CI 强制，因此接入另一个引擎或存储只需新增一个叶子 crate，不必改动核心。参见 [crate 依赖方向](./panel/README.md#crate-dependency-direction) 与 [扩展规则](./panel/README.md#extension-rules)。

控制台的各个功能模块自行注册路由与导航，彼此之间不能相互引用；共享的部分放在 `src/lib` 与 `src/components`。参见 [`panel/web/README.md`](./panel/web/README.md)。

# 构建

## 环境要求

- Rust 1.94 或更高版本；CI 还会用 1.98 与 nightly 构建
- 控制台需要 Node.js 22.18 或更高（或 24.12 或更高）以及 pnpm 11.22
- 构建镜像需要 Docker 或 Podman
- 可选：带 JetStream 的 NATS 服务，用于启动完整控制面的测试

`protoc` 已随构建依赖提供，生成代码在构建时产生，不提交到仓库。

## 命令

```bash
# 网关、控制面与命令行
cargo build --manifest-path panel/Cargo.toml --workspace --locked

# 控制台，由 panel-api 从 PINGORA_PANEL_WEB_ROOT 提供
cd panel/web && pnpm install --frozen-lockfile && pnpm build

# 镜像
docker compose -f panel/deploy/compose.yaml build
```

本地的标准验证流程：

```bash
cargo fmt --manifest-path panel/Cargo.toml --all --check
cargo clippy --manifest-path panel/Cargo.toml --workspace --all-targets -- -D warnings
PANEL_TEST_NATS_URL=nats://127.0.0.1:4222 cargo test --manifest-path panel/Cargo.toml --workspace --locked
for s in .github/scripts/check-panel-*.py; do python3 "$s"; done
bash .github/scripts/check-panel-boundaries.sh
(cd panel/web && pnpm type-check && pnpm lint && pnpm test:unit --run && pnpm test:e2e)
```

未设置 `PANEL_TEST_NATS_URL` 时，需要事件代理的测试会被跳过。参与开发前，请先阅读 [扩展规则](./panel/README.md#extension-rules) 和 [决策记录](./docs/adr/)。

# 安全与限制

- 管理接口绑定回环地址。控制台与 API 都需要登录；第一个管理员通过一次性引导令牌创建。要从其他地方访问控制台，可以把它作为 HTTPS 网站经由网关发布，或使用隧道。
- 密钥在主机上生成，作为 Compose secrets 挂载，API 从不返回它们。主密钥用于封存已保存的私钥。
- 控制面只连接你配置的服务：ACME 目录、DNS-01 使用的 DNS 服务器、OpenID Connect 身份提供方以及告警 Webhook。它不会向任何地方上报使用情况。
- 一个安装运行一个网关。限流、重试预算、熔断与排队都按网关进程计数。
- 暂不支持 HTTP/3 与脚本扩展。

# 项目状态

Pingora Panel 正在积极开发中，尚未到达 1.0。[`PRODUCT_SPEC.md`](./PRODUCT_SPEC.md) 列出了全部规划功能及其状态、路线图和 1.0 的质量门禁。

<p align="center">
  <a href="https://www.star-history.com/#eltavine/pingora-panel&amp;Date">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=eltavine/pingora-panel&amp;type=Date&amp;theme=dark" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=eltavine/pingora-panel&amp;type=Date" />
      <img alt="Star 历史图" src="https://api.star-history.com/svg?repos=eltavine/pingora-panel&amp;type=Date" />
    </picture>
  </a>
</p>

# 免责声明

本软件按"原样"提供，不附带任何形式的担保。运维者需自行负责对外暴露的内容、证书、密钥与备份，并在把变更应用到生产流量之前进行测试。开发者不对因使用本软件造成的损坏、数据丢失或服务中断承担责任。

# 许可证

Pingora Panel 采用 [Apache License 2.0](./LICENSE) 许可。

它基于 Cloudflare 的 [Pingora](https://github.com/cloudflare/pingora) 构建，Pingora 同样采用 Apache License 2.0。本仓库中的 Pingora crate 保留其原有的版权与许可声明，对它们的任何本地改动都记录在 [`docs/upstream-patches.md`](./docs/upstream-patches.md) 中。Pingora Panel 与 Cloudflare 不存在隶属或背书关系。
