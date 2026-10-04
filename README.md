# Pingora Panel

[![Panel Build](https://github.com/eltavine/pingora-panel/actions/workflows/panel.yml/badge.svg)](https://github.com/eltavine/pingora-panel/actions/workflows/panel.yml)
[![Security Audit](https://github.com/eltavine/pingora-panel/actions/workflows/audit.yml/badge.svg)](https://github.com/eltavine/pingora-panel/actions/workflows/audit.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Pingora Panel 是一个面向团队运维的单节点网站网关控制平台。项目计划以 Pingora 为数据面，以版本化 DSL 为规范配置源，通过同一组 REST API 为 Web GUI 与 `ppanel` CLI 提供站点、路由、上游、TLS、配置审批、原子发布、回滚、审计和可观测能力。

## 当前状态

> 当前 Pingora crates：0.9.0

**In Progress / durable gateway foundation。** `panel/` 独立 workspace 已提供 Proto-first 契约、稳定错误模型、领域值对象、Engine-neutral IR、`GatewayEngine`/`SnapshotStore` ports、内存 `FakeGatewayEngine`、Pingora 0.9.0 数据面适配器、独立 durable runtime、原子文件快照存储、Tonic gRPC transport、标准 gRPC Health 和 `gatewayd` 组合根。Prepare/Activate/CAS、持久 Activation Receipt、Last Known Good 重启恢复、v1/v2 磁盘格式 Golden Fixture、真实 TCP gRPC 黑盒闭环、`SERVING → NOT_SERVING → exit` 两阶段 drain、同端口重启、版本/uptime/worker 状态、plaintext loopback-only 管理端口以及适配器依赖隔离已有自动化测试。Proto breaking 门禁分别使用 PR 目标分支和 push 前一提交作为基线，并通过真实 Buf 自测试验证 bootstrap、additive、删除字段、改类型和复用编号路径。模块化 Axum/Utoipa REST adapter、独立 JSON compiler adapter、trait-object application service 和幂等激活回放 decorator 已完成第一条契约切片；`panel-api`、`config-service`、`automation-service`、`observability-service` 与 `audit-service` 作为模块运行在同一个控制面进程 `panel-control` 中，模块之间经进程内 gRPC 调用，各自提供聚合 Readiness、Degraded Mode、服务注册与协议版本协商；`panel-api` 在 loopback 公共 listener 上提供 REST 与 Web 控制台，并经 `config-service` 发布配置。领域事件采用 CloudEvents 1.0 契约；每个模块使用独立的 SQLite 文件（WAL 模式），提供事务 outbox 与幂等消费记录，outbox relay 按追加顺序发布到 NATS JetStream，持久消费者支持重试、死信队列与定向重放；`panel/web` 黑白 shadcn-vue Web 控制台已提供网关概览与数据面控制、网站、上游、监听与证书管理、配置发布与发布回执。控制面调用网关与主机代理时使用内部 CA 签发并自动轮换证书的 mTLS。`panel/deploy` 提供单镜像与 Docker/Podman Compose 单机安装。Pingora 数据面按生效配置提供 listener、虚拟主机、路由、TLS、静态内容与上游负载均衡和健康检查，支持平滑 reload 与 worker 调整；站点、域名、路由、上游、监听与证书配置以草稿形式编辑、校验并原子应用，REST API、`ppanel` CLI 与 Web 控制台提供同一套操作。草稿同时是以 `main.conf` 为入口的 Nginx 风格配置语言文件，可检查、格式化、补全、查看语法树、生成计划与 Diff、导出 IR 并试运行；每次应用都会记录可比较、可备注、可一键回滚的配置版本。每次变更与被拒绝或失败的尝试都由审计服务记录在防篡改的哈希链中，可在 API、CLI 与控制台按操作人、事件类型、关联 ID 与时间查询并校验完整性。所有请求都需要登录：首个管理员通过部署生成的一次性引导令牌创建，密码遵循 NIST SP 800-63B-4 并以 Argon2id 存储，浏览器会话使用 `__Host-` Cookie 与 CSRF 令牌，命令行使用 Bearer 会话或限定权限的 API 令牌，每个接口按权限目录与内置角色授权，登录与被拒绝的访问都进入审计日志。产品功能只有在满足规格验收条件后才会依次标记为 `In Progress`、`Implemented` 和 `Verified`。

完整产品边界、架构、接口、685 项功能目录、版本路线图和 1.0 质量门禁见 [PRODUCT_SPEC.md](PRODUCT_SPEC.md)。该文件是产品需求的唯一权威来源。

`gatewayd::management_router` / `management_router_with_config` 已提供 REST→JSON compiler→application→gateway 的共享组合工厂；它与 gRPC 使用同一个 `GatewaydEngine`，并强制注入幂等 repository、在直连 engine 前以纳秒精度校验 deadline。HTTP listener 仍需在认证/mTLS policy 完成后显式绑定。

当前 REST 适配器包含预备快照取消接口；JSON IR 拒绝未知字段和无效领域值，明文 gRPC 客户端只连接数字 loopback 地址。CI 使用固定版本的 oasdiff 对 OpenAPI 进行跨提交兼容检查，保留单独的 fixture 一致性测试。

Initial foundation 构建：

```text
cargo fmt --manifest-path panel/Cargo.toml --all -- --check
cargo check --manifest-path panel/Cargo.toml --workspace --locked
cargo test --manifest-path panel/Cargo.toml --workspace --locked
cargo clippy --manifest-path panel/Cargo.toml --workspace --all-targets --all-features -- -D warnings
```

Proto 使用 `panel/proto` 作为唯一输入，Rust 文件在构建时动态生成到 `OUT_DIR`，不会提交生成物。Buf lint/breaking、边界守卫和 Pingora 适配器 smoke test 由独立的 `.github/workflows/panel.yml` 执行。

本地启动内部 Gateway gRPC 服务：

```text
PINGORA_PANEL_STATE_DIR=/var/lib/pingora-panel/gateway \
PINGORA_PANEL_GATEWAY_ADDR=127.0.0.1:50051 \
PINGORA_PANEL_WORKERS=4 \
PINGORA_PANEL_DRAIN_TIMEOUT_MS=1000 \
cargo run --manifest-path panel/Cargo.toml --package gatewayd
```

在内部 mTLS transport 完成前，`PINGORA_PANEL_GATEWAY_ADDR` 只接受 IPv4/IPv6 loopback；非 loopback plaintext 地址会 fail closed。worker 数必须位于 `1..=256`，drain timeout 最大为 300 秒。

具体 crate 依赖方向、激活顺序和扩展规则见 [`panel/README.md`](panel/README.md)。
当前进程能力、readiness 与故障诊断见 [Gateway foundation 运维说明](docs/gateway-foundation-runbook.md)。

## Pingora 上游归属

本仓库保留并基于 Cloudflare 的 [Pingora](https://github.com/cloudflare/pingora) 开源项目进行开发。Pingora 是用于构建可编程网络系统与代理服务的 Rust 框架，其上游代码采用 [Apache License 2.0](LICENSE)。Pingora Panel 将通过独立适配器隔离上游 API，保留原始版权、许可和修改记录；项目与 Cloudflare 不存在官方隶属或背书关系。
