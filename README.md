# Pingora Panel

[![Panel Build](https://github.com/eltavine/pingora-panel/actions/workflows/panel.yml/badge.svg)](https://github.com/eltavine/pingora-panel/actions/workflows/panel.yml)
[![Security Audit](https://github.com/eltavine/pingora-panel/actions/workflows/audit.yml/badge.svg)](https://github.com/eltavine/pingora-panel/actions/workflows/audit.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Pingora Panel 是一个面向团队运维的单节点网站网关控制平台。项目计划以 Pingora 为数据面，以版本化 DSL 为规范配置源，通过同一组 REST API 为 Web GUI 与 `ppanel` CLI 提供站点、路由、上游、TLS、配置审批、原子发布、回滚、审计和可观测能力。

## 当前状态

**In Progress / durable gateway foundation。** `panel/` 独立 workspace 已提供 Proto-first 契约、稳定错误模型、领域值对象、Engine-neutral IR、`GatewayEngine`/`SnapshotStore` ports、内存 `FakeGatewayEngine`、Pingora 0.8.0 数据面适配器、独立 durable runtime、原子文件快照存储、Tonic gRPC transport、标准 gRPC Health 和 `gatewayd` 组合根。Prepare/Activate/CAS、持久 Activation Receipt、Last Known Good 重启恢复、v1 磁盘格式 Golden Fixture、真实 TCP gRPC 黑盒闭环以及适配器依赖隔离已有自动化测试。控制服务、PostgreSQL、NATS、REST API、`ppanel` CLI、Web GUI 和生产 listener 尚未实现；产品功能只有在满足规格验收条件后才会依次标记为 `In Progress`、`Implemented` 和 `Verified`。

完整产品边界、架构、接口、685 项功能目录、版本路线图和 1.0 质量门禁见 [PRODUCT_SPEC.md](PRODUCT_SPEC.md)。该文件是产品需求的唯一权威来源。

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
cargo run --manifest-path panel/Cargo.toml --package gatewayd
```

具体 crate 依赖方向、激活顺序和扩展规则见 [`panel/README.md`](panel/README.md)。

## Pingora 上游归属

本仓库保留并基于 Cloudflare 的 [Pingora](https://github.com/cloudflare/pingora) 开源项目进行开发。Pingora 是用于构建可编程网络系统与代理服务的 Rust 框架，其上游代码采用 [Apache License 2.0](LICENSE)。Pingora Panel 将通过独立适配器隔离上游 API，保留原始版权、许可和修改记录；项目与 Cloudflare 不存在官方隶属或背书关系。
