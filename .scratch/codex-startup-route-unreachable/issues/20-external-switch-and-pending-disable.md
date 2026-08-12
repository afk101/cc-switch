# 20 — 修正 External 切换与 pending disable 状态判定

Status: resolved

**构建内容：** 修复 Acceptance 复审剩余的两个状态机边界：External 显式切换不得嵌入不可证明旧备份或重写外部所有权；pending disable 恢复需区分 Home 已恢复与仍受管，并在 stop 后复检。

**受阻于：** 19 — 脱敏嵌套旧备份并恢复中断关闭。

## Acceptance Criteria

- [x] enabled route 的显式 switch 预检为 External 时，以当前外部 Home 建立全新安全基线，旧 v1/v2 backup 不嵌入组合 PREPARED JSON。
- [x] External 切换失败/崩溃恢复后仍保持外部 Home/External 语义，不把外部路径 rebase 为 RouteOwned；成功切换按用户显式接管建立新 Managed backup。
- [x] 旧 backup 含轮换 listener token 时，External switch 的任意持久化 JSON 不含可逆旧/新 token。
- [x] pending disable 若 Home 已恢复为 previous baseline、仅 stop/DB 阶段失败，重试保留 Managed 并继续关闭，不误判 External。
- [x] pending disable 若本轮恢复 Home，async stop 后再次复检；stop 期间外部严格编辑收敛 External，瞬态 I/O/DB 失败可重试。
- [x] 日志 operation 使用既有 disable 常量，测试断言同步复用。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，External switch 与 pending disable 两组公开 manager seam 先 RED。
- External Home 只在用户显式成功接管时转 Managed；失败补偿不得改变所有权事实。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- External 显式切换在已有 token 预检后继续校验完整活动严格路径；一旦确认 External，操作起点路由保留 `External` 且丢弃不可证明旧 backup，PREPARED 只携带当前外部 Home 的全新脱敏基线。崩溃/失败恢复保持外部 Home 与 `External`，只有完整成功提交才转为 `Managed`。
- Home 服务新增关闭三态 `CurrentRoute` / `PreviousBaseline` / `External`。pending disable 先识别已经恢复的 previous baseline，只继续 stop 与路由落库；若仍是当前路由则恢复，若是 External 则静默收敛，避免用字段形状猜测。
- pending disable 在 async stop 完成后再次执行同一三态检查；stop 尾窗外部严格编辑收敛 `External`，已有 I/O/DB 重试回归保持通过。生命周期日志及测试断言统一复用 `CODEX_ROUTE_RECOVERY_OPERATION_DISABLE`。
- TDD 两组公开 manager seam 均先 RED 后 GREEN。验证：RouteManager 123/123、Codex Profile 200/200、Rust lib 2511 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中 Issue20 修改文件无报告；仅剩未修改文件中的 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项。
