# 13 — 已存在新 secret 时仍用已证明旧 token 脱敏

Status: resolved

**构建内容：** 修复最终 ship Spec 复审的对称分支：secret 已存在但不同于 Home 中可证明的旧 token 时，v1/v2 rebase 仍必须携带旧 token 脱敏历史正文。

**受阻于：** 12 — 使用旧 token 脱敏历史备份再升级。

## Acceptance Criteria

- [x] 手工真实 v1/v2 backup 的 `previous_content` 含旧 listener token O，Home/旧 proof 可证明 O，而 secret store 已存在新 token N 时，startup 安全升级为 v3。
- [x] RouteOwned 分类把已证明的 Home 旧 token 以内存专用类型交给 rebase；旧 token O 脱敏正文，新 token N 生成 proof。
- [x] 解码新 backup 不含 O 或 N，Home 只更新严格 token并保留非路由字段，runtime 启动。
- [x] 未证明、外部 token、当前 token一致、secret缺失分支保持既有安全语义。
- [x] Rust、前端、格式、Clippy 归因与相关全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，手工构造真实 v1/v2，禁止当前 writer helper。
- O 只存在短生命周期内存，不得持久化、Debug、日志或错误。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD RED 以公开 `CodexRouteManager` 启动 seam 复现：真实手工 v1 的 Home/target 已证明旧 token O、secret 已有 N 时，Home 虽切到 N，但升级备份仍为 `embedded`，可逆保留 O。
- 已有 secret 的启动预检现在只在 Home token 与当前 secret 不同时复用历史 ownership proof；证明成功后以既有不可序列化、`Debug` 固定脱敏的专用类型短暂携带 O，当前 token 一致时不携带，无法证明时仍按 External/fail-closed 收敛。
- 手工真实 v1 target 与 v2 字段级 proof 两条回归均 GREEN：Home 只把严格 token 更新为 N，保留 model、Desktop 与 plugins，未调用 `ensure_token`，runtime 启动；解码后的 v3 `previous_content` 与完整 JSON 均不含 O/N。
- 验证：RouteManager 101/101、Codex Profile 193/193、Rust lib 单线程全量 2488 passed/2 ignored、前端 Vitest 单 worker 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件的 8 个既有 lint：migration 1 项、body_dump 6 项、forwarder 1 项；Issue 13 修改文件无报告。
