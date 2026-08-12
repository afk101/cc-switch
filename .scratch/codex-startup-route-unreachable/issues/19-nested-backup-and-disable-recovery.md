# 19 — 脱敏嵌套旧备份并恢复中断关闭

Status: resolved

**构建内容：** 修复认证复审的两个阻断项：显式切换嵌入历史 backup 前先升级脱敏；中断关闭重启恢复时重新预检并使用可证明旧 token。

**受阻于：** 18 — 测试故障注入统一 recovery operation 常量。

## Acceptance Criteria

- [x] 真实 v1/v2 live backup 在显式切换生成组合 PREPARED backup 前升级脱敏，整份持久化 JSON 不含旧/新 listener token 的明文或可逆字节。
- [x] 切换在 PREPARED/route-write 后崩溃，恢复仍能还原旧 live backup 的安全语义且不重新引入 token。
- [x] disable PREPARED/home_restore_failed 崩溃重启时重新执行只读 preflight；secret=N、Home=可证明旧 O 时将 O 以内存专用类型交给恢复。
- [x] 中断关闭可最终恢复接管前严格字段、停止 runtime、清 recovery/backup 并 disabled；无法证明时按 External/fail closed 语义处理。
- [x] v1/v2/v3、Profile 隔离、脱敏日志及故障重试回归通过。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，必须使用手工真实 v1/v2 envelope 和新 manager 崩溃恢复 seam。
- 旧 token 只允许短生命周期内存，不得进入嵌套 backup/recovery/DB/log/error。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 显式切换在构造任何组合备份前重新执行现有只读 token preflight；Home 中可证明旧 token `O` 仅以 `CodexProvenPreviousListenerToken` 在内存中传递，历史 live backup 与操作前精确备份都先用 `O` 脱敏，再以当前 token `N` 生成新目标 proof。
- 嵌入的 v1/v2 live backup 升级为 v3，移除上一轮瞬态嵌套字段；PREPARED 与 route Home 已落盘两个崩溃窗口的测试均递归解码 JSON 字符串和数字字节数组，证明 `O`、`N` 都不可还原。route-write 补偿用额外不可逆恢复指纹校验以 `N` 重建的安全 Home，未放松完整性门禁。
- disable `prepared` 与 `disable_home_restore_failed` 恢复不再直接拿当前 secret 猜所有权；新 manager 会重跑 `prepare_managed_home_for_disable`，把已证明 `O` 交给字段恢复。成功后恢复接管前严格字段、停 runtime，并清理 backup/recovery；External 走既有静默关闭，瞬态 I/O 保留可重试状态。
- TDD：切换 RED 为旧实现尚未进入 PREPARED 就报 listener token 冲突；关闭恢复 RED 为补偿未收敛。修复后两条公开 manager seam 均 GREEN，另有 Home seam 参数化覆盖手工 v1/v2。
- 验证：Home 39/39、RouteManager 121/121、Codex Profile 214/214、Rust lib 单线程全量 2509 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中本 issue 修改文件无报告；仅剩未修改文件的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项。
