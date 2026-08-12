# 16 — 收口切换、启停与诊断原子性窗口

Status: resolved

**构建内容：** 修复终审剩余的生命周期原子性窗口：已启用切换写前持久化、enable prepared 完整补偿、disable 后验校验、外部接管 DB 失败不留提示，并归位模型目录字段常量。

**受阻于：** 15 — 关闭重分类与启用写前基线。

## Acceptance Criteria

- [x] 已启用供应商切换从同一原始 snapshot 构造 latest backup/catalog/route 计划，并在任何 Home/catalog 写入前持久化 recovery+backup。
- [x] 切换在 catalog 写后、route 写前或 runtime swap 前崩溃，恢复可完整回到本次操作前 Home/catalog/route/runtime，不退回更早基线。
- [x] enable 在首次 catalog/Home 写前进入可恢复阶段；`prepared`/`enable_started` 崩溃恢复覆盖 config 与 catalog 文件前态，不遗留指针或目录。
- [x] disable restore 成功后、最终 disabled 保存前再次校验 Home；此窗口发生外部严格字段编辑时保持用户 Home，收敛 disabled/External 并清 stale backup/recovery。
- [x] 外部接管自动关闭 DB 保存失败时 route 原样、runtime 不启动，仅写脱敏应用日志；不得持久化 `last_error` 或 Home 路径提示。
- [x] `CODEX_MODEL_CATALOG_FIELD` 移入 Codex Profile 常量文件并统一导出，功能不变。
- [x] 既有启停/切换/恢复/兼容/Profile 隔离/脱敏语义保持，Rust、前端、格式、Clippy 归因及全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，四个行为均建立公开 manager crash/race/failure seam RED。
- 任何 Home/catalog 写入前必须已有足以完整补偿的持久化记录。
- 不新增 watcher，不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## 验证记录

- 四个公开 manager seam 已按 RED → GREEN 完成：切换 catalog 写后崩溃、enable `prepared`/`enable_started` 崩溃、disable restore 后外部编辑、启动外部接管 DB 保存失败。
- `cargo test --lib codex_profile::route_manager::codex_route_manager --quiet`：114 passed。
- `cargo test --lib --quiet -- --test-threads=1`：2501 passed，2 ignored。
- `pnpm exec vitest run --exclude scripts/upgrade.test.js --pool=forks --poolOptions.forks.singleFork=true`：637 passed。
- `pnpm typecheck` 与 `pnpm build:renderer`：通过。
- `cargo fmt --check`、`git diff --check`：通过。
- `cargo clippy --lib -- -D warnings`：Issue 16 修改文件无告警；仓库既有 8 个告警位于 `migration.rs`、`proxy/body_dump.rs`、`proxy/forwarder.rs`。
