# 03 — 多 Profile 显式 Sync、Import 与云恢复

Status: resolved

**构建内容：** 手动 Sync、数据库导入与云恢复完成后，逐个 Managed Profile 按自己的主供应商同步 model family；单个失败不阻塞其他 Profile，并向用户返回脱敏的部分成功警告。

**受阻于：** 01 — Model family 权威投影原语。

## 覆盖范围

- Requirements：REQ-09、REQ-11、REQ-12、REQ-13、REQ-15、REQ-16。
- Scenarios：SCN-12、SCN-13、SCN-14、SCN-15、SCN-16、SCN-18。
- 本 slice 贯通 route-aware manager、现有 Sync/Import/Restore 命令结果与用户可见 warning。

## Acceptance Criteria

- [ ] 手动 Sync 遍历所有 Managed Profile，并按各自的有效主供应商同步 model family。
- [ ] Import 与云恢复在 DB 成功后触发同一 route-aware Profile 同步。
- [ ] External 跳过；没有主供应商或单个 Home 失败时记录该 Profile 结果并继续。
- [ ] 已成功 Profile 不因后续失败回滚；Import/Restore 的 DB 成功不回滚。
- [ ] 全成功返回成功；部分失败返回完成状态与逐 Profile 脱敏 warnings。
- [ ] warning 不包含 token、API Key 或配置正文，并能让用户识别需要修复的 Profile。
- [ ] 普通 startup 继续保留 model family；下一次明确 Sync 可以修复上次失败或崩溃残留。
- [ ] 先在 route-aware explicit sync public seam 观察 RED，再完成 GREEN；命令层只测试委托和公开结果。

## 验证方式

- 运行 route-aware explicit sync 的多主供应商、External、缺失主供应商、写失败继续与 warning 测试。
- 运行数据库 Import 和云恢复的全成功、部分失败用户结果测试。
- 运行现有 global Live sync、import/export 与 Profile startup 对账回归。
- 运行前端 typecheck、相关测试、Rust formatter 与静态检查。

## 执行约束

- 修改测试或生产代码前调用 `$tdd`。
- 使用结构化 outcome 表达部分成功，不从日志文本反向解析结果。
- 不将 best-effort 扩散到交互式 provider save。
- 不新增 journal；不让 startup 自动修复 model-family 残留。
- 失败摘要必须在靠近错误源的位置脱敏，并保持现有 Import/Restore DB 成功语义。

## 范围之外

- Provider save 的全引用原子扇出和 provider key rename。

## Comments

- 若现有云恢复入口有多个调用方，所有 post-import 路径必须汇入同一显式同步 contract。
- RED：`explicit_sync_uses_each_managed_profile_primary_provider_and_skips_external` 首次因 `CodexRouteManager::sync_managed_profiles_explicit` 不存在而编译失败，证明 route-aware public seam 尚未建立。
- GREEN：新增逐 Profile 显式同步结果与 manager seam；多主供应商分别应用有效 model family，External 以结构化 outcome 跳过。
- RED：`explicit_sync_continues_after_profile_failure_and_returns_sanitized_warning` 首次因 warning 缺少独立 `reason` 字段失败。
- GREEN：新增专用 `CodexProfileSyncWarning`；单 Profile Home 失败与缺少主供应商均继续后续 Profile，warning 只含 Profile 标识、名称、Home 路径和固定脱敏原因。
- RED/GREEN：命令 payload 测试先因 `attach_profile_sync_result` 不存在而编译失败；实现后保持 Import/Restore 成功字段并附加 `profileSync`、`warnings` 结构。
- 调用方：手动 Sync、SQL Import、数据库备份恢复、WebDAV download、S3 download 全部汇入 `run_post_import_sync`；Import 前端移除重复 Sync，所有恢复界面展示逐 Profile 名称与脱敏原因。
- 验证通过：3 个 route-aware manager 测试、2 个 sync payload 测试、26 个 `import_export_sync` 回归、provider-save 原子补偿回归、startup 保留用户 model-family 回归、7 个相关前端测试、`pnpm typecheck`、Rust `cargo check --tests`、`cargo fmt --check`、`git diff --check`。
- Clippy：`cargo clippy --all-targets` 通过但报告 10 个既有 warning；`-D warnings` 因 `migration.rs`、`proxy/body_dump.rs`、`proxy/forwarder.rs`、`prompt_files.rs` 及 route manager 既有测试行失败，本 issue 新增代码没有 Clippy warning。
- 前端全量：754/755 测试通过；既有 `scripts/upgrade.test.js` URL scheme suite 问题与 `tests/integration/App.test.tsx` 5 秒超时导致命令失败，目标前端测试使用精确 Vitest 入口全部通过。超大全量最终归因留给 Issue 04。
