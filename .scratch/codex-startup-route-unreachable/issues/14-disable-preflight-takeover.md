# 14 — 显式关闭前重新识别外部接管

Status: resolved

**构建内容：** 修复最终 Spec 复审发现的显式关闭缺口：路由开启后用户在本次运行期间修改严格字段，点击关闭前必须重新分类，而不是依赖启动时的旧 `Managed` 状态。

**受阻于：** 13 — 已存在新 secret 时仍用已证明旧 token 脱敏。

## Acceptance Criteria

- [x] route=enabled/Managed 时，用户外部修改当前 Home 严格字段后直接点击关闭，操作前识别为 External。
- [x] External 关闭保持 Home 字节不变，清 stale backup/recovery，标记 disabled/External，保留 provider/failovers，并停止 runtime。
- [x] 预检必须发生在写 disable recovery 前；不得留下无意义 recovery 或错误提示。
- [x] 普通 Managed 关闭仍恢复最近接管前严格字段；瞬态 I/O/无 proof 保持 fail closed，不误关或覆盖 Home。
- [x] runtime stop/DB save 失败的补偿与重试语义保持一致且脱敏。
- [x] Rust、前端、格式、Clippy 归因与相关全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，用真实 enable→外部编辑→disable 公开 manager seam 先 RED；禁止测试手工预设 External。
- 不增加 watcher；只在显式关闭操作前检查一次。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD RED 通过真实 `CodexRouteManager` 生命周期复现 `enable → 用户修改活动 base_url → disable`：旧实现先写 `disable/prepared` recovery，再因严格字段冲突失败。
- 显式关闭现在在任何 recovery、Home 或 route 写入前执行只读字段级所有权预检；识别 External 后先停止 runtime，再以单条 route 保存收敛为 disabled/External，清理 stale backup/recovery/error，保留主供应商与故障转移引用，Home 字节不变。
- stop 失败、数据库保存失败、瞬态 Home I/O 与缺失 secret 均保留 enabled/Managed、backup 与 Home 供重试，不创建无意义 recovery；应用日志只记录脱敏的 operation/profile/stage/category。
- 对称兼容回归覆盖 Home 仍使用可证明旧 token、私有 secret 已轮换的关闭路径：旧 token 只作为不可序列化的短生命周期所有权证明，恢复后新旧 listener token 均不进入 Home、备份或日志。
- 验证：RouteManager 106/106、Codex Profile 182/182、Rust lib 2493 passed/2 ignored、TypeScript typecheck、前端 Vitest 93 files/637 tests、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件的 8 个既有 lint：migration 1 项、body_dump 6 项、forwarder 1 项；本 issue 修改文件无报告。
