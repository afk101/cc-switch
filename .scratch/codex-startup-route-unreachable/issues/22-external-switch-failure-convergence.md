# 22 — External 切换失败静默关闭

Status: resolved

**构建内容：** 修复 release certificate 发现的语义分裂：External 起点的显式切换失败/崩溃恢复后不得保留 enabled+External 与旧 runtime，必须静默收敛为 disabled/External。

**受阻于：** 21 — 收口 External 切换尾窗原子性。

## Acceptance Criteria

- [x] External origin switch 任一失败补偿完成后停止并移除 runtime，route 原子保存 disabled/External，保留 provider/failovers，清 backup/recovery/error。
- [x] External origin switch 崩溃后新 manager 恢复得到同一 disabled/External 稳定态，Home 外部配置字节不变。
- [x] 不留 UI 提示或持久 last_error，不生成/泄漏 listener token。
- [x] switch 成功 final clear 仍原子提交 enabled/Managed+新 backup/runtime。
- [x] Managed origin switch 失败仍恢复 enabled/Managed 与旧 runtime，不受影响。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，失败补偿和崩溃重启两条 External switch seam 先 RED。
- External 失败收敛不得写 Home，不增加提示或 watcher。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 两条公开 manager seam 均先 RED：External 起点在 failovers 阶段失败补偿后仍为 `enabled/External`；final-clear 崩溃后的新 manager 恢复同样保留该不可服务状态。修复后两条均收敛为 `disabled/External`。
- External 切换补偿与 pending recovery 共用单一收敛 helper：先恢复操作前 Home 与模型目录，运行时存在时立即拒绝新请求并停止，再恢复原故障转移引用，最后清理 backup/recovery/error 并保存关闭态；真实新进程没有旧 runtime 时直接继续收敛。
- Home 外部配置逐字节保持，主供应商与故障转移引用保留；持久化内容递归检查不含新旧 listener token，也不创建 UI 提示或 `last_error`。补偿存储持续失败时保留 External pending recovery 供下一次重试，且不再次覆盖 Home。
- 对称回归证明 Managed 起点的旧 enabled/runtime 回滚不变；External 成功切换仍原子提交 `enabled/Managed`、新 backup、新 provider/failovers runtime snapshot。
- 验证：RouteManager 128/128、Codex Profile 221/221、Rust lib 单线程全量 2516 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中 Issue 22 修改文件无报告；仅剩未修改文件的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项。对这四类既有 lint 显式 allow 后严格 Clippy 通过。
