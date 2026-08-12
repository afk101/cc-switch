# 23 — External 切换 PREPARED 前失败收敛

Status: resolved

**构建内容：** 修复 Signoff 剩余窗口：External 已识别后，计划构建或备份序列化在 PREPARED 前失败也必须静默关闭，不留下 enabled/Managed 假状态；复用统一 External 收敛 helper。

**受阻于：** 22 — External 切换失败静默关闭。

## Acceptance Criteria

- [x] External origin switch 在计划构建、token 脱敏或 backup 序列化的任一 PREPARED 前错误时，Home 原样、runtime 停止移除、route disabled/External。
- [x] 收敛保留 provider/failovers，清 backup/recovery/error，不持久化提示或 token。
- [x] 活动用户 token + 非活动 provider 残留 listener token 的无法无损表达场景有公开 manager RED→GREEN。
- [x] External 收敛复用 `persist_external_takeover`，不重复构造关闭态；存储/stop 失败保持可重试。
- [x] Managed switch、External 成功与已进入 PREPARED 的失败语义保持。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，PREPARED 前失败 seam 先 RED。
- 不写 Home、不新增提示或 watcher。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 公开 manager seam 先稳定 RED：活动 provider 使用用户 external base/token、非活动 provider 残留当前 Profile listener token 时，旧实现的备份脱敏在 PREPARED 前返回“无法无损表达”，但留下 `enabled/Managed` 与旧 runtime；修复后 Home 字节不变并收敛为 `disabled/External`。
- External 一经分类，切换计划构建、组合备份序列化、recovery 编码和首次 PREPARED 保存被统一纳入准备结果；该区间失败通过固定脱敏日志进入统一 External 收敛，不影响尚未分类或 Managed 起点的既有错误语义。
- 通用收敛 helper 先拒绝并停止 runtime，再按需恢复旧 failovers，最后复用 `persist_external_takeover` 原子提交关闭态并移除 runtime；Issue 22 不再重复构造 disabled route。
- stop 或 External 关闭态保存失败时保留原 route、backup 与 runtime 登记供下次操作重试，不写 Home、不生成 recovery/提示；对应两条公开 manager 重试回归均通过。
- 验证：RouteManager 132/132、Codex Profile 224/224、Rust lib 单线程全量 2519 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中本 issue 修改文件无报告；仅剩未修改文件中的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项。对四类既有 lint 显式 allow 后严格 Clippy 通过。
