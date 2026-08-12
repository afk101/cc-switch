# 28 — 自动投影权威同步 provider 受管字段

Status: resolved

**构建内容：** 修复 Platinum Spec 剩余缺口：自动直连投影保留用户 model/reasoning，但活动 provider 的受管字段必须权威同步，供应商撤销 token/base 等字段时应从 Home 删除。

**受阻于：** 27 — 自动直连投影保留用户模型。

## Acceptance Criteria

- [x] 自动直连投影仅保护顶层 `model`、`model_reasoning_*` 及明确用户扩展字段。
- [x] 活动 provider 的受管字段（含 base_url、wire_api、experimental_bearer_token 等）按目标权威替换；目标缺失时删除旧字段。
- [x] 共享供应商删除 token 后，disabled/Managed Home 不再保留旧 token，同时用户 model/reasoning/unknown/Desktop 保留。
- [x] 启动自动对账同样遵循该规则；External 跳过、显式 different-provider model 授权不变。
- [x] inline/标准 provider table、回滚、Profile 隔离和脱敏回归通过。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，共享保存撤销 token 与启动对账两个公开 manager seam 先 RED。
- 不使用整份 Home 覆盖；只权威管理明确 provider 派生字段。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 的两条公开 manager seam 均先稳定 RED：共享供应商保存会保留目标已撤销的标准表 token；启动对账会整个替换 inline provider table 并丢失用户扩展。GREEN 后两种 TOML 形态使用同一 `TableLike` 投影语义。
- Codex 标准 provider 字段以常量化白名单表达权威子集：目标声明则整字段替换（包括嵌套 headers/query 表），目标撤销则删除；未知 provider 扩展、顶层模型/reasoning/Desktop 与其他用户字段保留。顶层严格路由 fallback 只在它会成为目标有效路径时同步。
- 回归覆盖标准表、inline table、provider 受管字段撤销、用户扩展保留、嵌套受管表整体替换、跨 Home 写入失败逐字节回滚、External 跳过、显式切换和 Profile 隔离。
- 验证：RouteManager 137/137、Codex Profile 230/230、Rust lib 单线程全量 2525 passed/2 ignored、前端 Vitest 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件中的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项；对四类既有 lint 显式 allow 后严格 Clippy 通过，Issue 28 修改文件无告警。
