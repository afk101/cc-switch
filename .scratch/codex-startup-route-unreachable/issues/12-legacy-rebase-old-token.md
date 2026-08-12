# 12 — 使用旧 token 脱敏历史备份再升级

Status: resolved

**构建内容：** 修复 ship Spec 复审的最后一个旧格式升级漏洞：secret 丢失后的 v1/v2 自愈必须用已证明的旧 Home token 脱敏历史 `previous_content`，再用新 token 生成 v3 proof。

**受阻于：** 11 — 在 token 创建前完成外部接管预检。

## Acceptance Criteria

- [x] 真实 v1 与 v2 backup（非当前 helper 生成的 v3）在 `previous_content` 含旧 listener token、secret 丢失时，可由 Home/proof 证明后安全自愈。
- [x] 预检在内存携带已证明的旧 Home token，仅用于脱敏历史正文与解析旧路径；不得持久化或记录。
- [x] rebase 用旧 token 移除可逆历史值，用新 token 生成当前 v3 ownership proof；解码升级后 backup 不含新旧 token。
- [x] 无法证明旧 token 的 v1/v2 保持 External/fail closed，不创建 token、不覆盖 Home。
- [x] v3、legacy placeholder、Profile 隔离、日志脱敏与 runtime 启动回归通过。
- [x] Rust、前端、格式、Clippy 归因与相关全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，必须手工构造真实 v1/v2 envelope，禁止用当前 writer helper 冒充旧格式。
- 旧 token 只能存在于测试输入与短生命周期内存，不得进入新 backup、DB、日志或错误。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 预检新增不可序列化且 `Debug` 固定脱敏的短生命周期旧 token 证明值；它只从已通过 Home/历史 proof 验证的路径产生，并只传到本次启动 rebase。
- rebase 分离“历史正文脱敏 token”与“当前目标 proof token”：旧 token 仅移除 v1/v2 `previous_content` 中的可逆值，新 token 仅用于当前 v3 ownership proof 与 Home 路由目标。
- 手工构造的真实 v1 与 v2 envelope 均完成 secret 丢失启动自愈，解码后的 v3 backup 不含旧 token 或新 token；无法证明的手工 v1/v2 在 `ensure_token` 前静默收敛 External，Home 字节不变且 runtime 不启动。
- 验证：RouteManager 99/99、Codex Profile 175/175、Rust lib 单线程全量 2486 passed/2 ignored、前端 Vitest 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- Rust 并行全量唯一 `model_pricing` 临时文件时序测试失败，隔离 exact 与单线程全量均通过；`cargo clippy --lib -- -D warnings` 仅报告未修改文件的 migration 1 项、body_dump 6 项、forwarder 1 项，共 8 项既有 lint，Issue12 修改文件无报告。
