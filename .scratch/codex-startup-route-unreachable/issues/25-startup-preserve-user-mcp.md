# 25 — 启动恢复保留用户 MCP

Status: resolved

**构建内容：** 修复最终认证剩余缺口：enabled Profile 启动恢复不得整表替换 `mcp_servers`，只修复严格路由字段并保留用户/Codex 新增 MCP、注释及未知字段。

**受阻于：** 24 — External 关闭态保存失败后的 runtime 重试。

## Acceptance Criteria

- [x] enabled Profile 启动恢复目标不调用整表 MCP 投影；只修改 base_url、wire_api、listener token 等严格路由字段。
- [x] 用户已有且未登记于 CC Switch DB 的 MCP、表内注释和未知字段启动后逐字节/语义保留。
- [x] CC Switch DB 中的 MCP 派生同步若仍需要，保持为独立流程，不作为 listener 启动门禁，不覆盖用户 MCP。
- [x] 已有 DB MCP、模型目录、正常 runtime 启动与 disabled 派生同步语义按 spec 保持。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，真实 startup 两阶段 seam 先 RED，配置同时含用户 MCP、注释及 DB MCP。
- 不借启动恢复删除或重排用户 MCP，不增加 watcher。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 的真实两阶段 startup seam 先稳定 RED：Home 同时含用户维护 MCP、Codex 新增 MCP、注释与未知字段，数据库另有 Playwright MCP；旧启动恢复把 `mcp_servers` 整表替换为数据库投影，用户内容消失。
- `build_restore_plan` 现只用数据库有效供应商配置构造 runtime snapshot；Home 目标统一通过严格路由计划构造，不再把数据库 MCP 投影混入 listener 启动门禁。轮换 listener token 的 GREEN 回归证明发生真实严格字段写入后，用户 MCP 正文、注释、模型与其他非路由字段仍保留，runtime 状态为 `Running`。
- 独立派生语义保持：关闭态启动对账与关闭态共享供应商保存仍使用原有数据库 MCP 投影；对应既有测试继续验证 Playwright MCP、通用配置与模型目录收敛，enabled 启动不会借此覆盖 Home。
- 验证：RouteManager 135/135、Rust lib 单线程全量 2523 passed/2 ignored、前端 Vitest 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件中的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项；对四类既有 lint 显式 allow 后严格 Clippy 通过，Issue 25 修改文件无告警。
