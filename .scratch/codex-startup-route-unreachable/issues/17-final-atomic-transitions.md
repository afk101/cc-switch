# 17 — 收口阶段推进与异步尾窗

Status: resolved

**构建内容：** 修复终审剩余的原子阶段与异步尾窗，并清理组合计划重复逻辑和 recovery operation 裸字符串。

**受阻于：** 16 — 收口切换、启停与诊断原子性窗口。

## Acceptance Criteria

- [x] 已启用切换在 route Home 写成功、runtime swap/阶段推进前崩溃时，PREPARED recovery 可恢复 Home+catalog+route+backup/runtime 全部本次操作前状态。
- [x] enable 的 recovery 与本次新 backup 单次原子保存，不存在先存 recovery、仍指旧 backup 的崩溃窗口。
- [x] disable 在 async runtime stop 完成后、最终 route 保存前再次只读校验；stop 期间外部编辑收敛 disabled/External 并清 stale backup/recovery。
- [x] 瞬态 I/O、stop/DB 保存失败保持可重试且不覆盖 Home。
- [x] Home 组合计划共享流程提取为单职责 helper；recovery operation tag 使用 constants，不新增裸字符串比较。
- [x] 既有启停/切换/崩溃恢复/Profile 隔离/脱敏语义保持，Rust、前端、格式、Clippy 及全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，三个时序窗口均用公开 manager seam 先 RED。
- 阶段与 backup 的持久化边界必须足以让下一进程唯一判断并补偿。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 三条公开 manager seam 均先 RED：切换 Home 落盘后崩溃会残留新 model；enable 暴露 recovery+旧 backup 分裂保存；async stop 期间外部编辑最终仍为 Managed。
- 切换组合 backup 新增仅在操作途中存在的脱敏精确 Home 快照；PREPARED 恢复使用 CAS 幂等恢复 Home 与 catalog，然后回滚 route、backup、failovers 与 runtime snapshot，成功提交时清除瞬态字段。
- enable 使用一次 route save 同时持久化 recovery 与新 backup；disable 在 stop 后先推进 `disable_stopped`，再执行最终只读校验，I/O 和 DB 故障的二次操作均已证明可收敛。
- 验证：RouteManager 119/119、Home 38/38、Rust lib 2506 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项；Issue 17 修改文件无告警。
