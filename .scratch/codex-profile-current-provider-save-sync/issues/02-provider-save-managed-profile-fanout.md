# 02 — Provider 保存同步全部 Managed 主引用

Status: resolved

**构建内容：** 用户保存 Codex 供应商时，在返回成功前让所有 Managed 主引用 Profile（enabled 与 disabled）立即采用有效 model family，同时保持路由、runtime、目录、扩展与原子补偿合同。

**受阻于：** 01 — Model family 权威投影原语。

## 覆盖范围

- Requirements：REQ-01、REQ-06、REQ-07、REQ-08、REQ-09、REQ-14、REQ-15、REQ-17。
- Scenarios：SCN-01、SCN-02、SCN-06、SCN-07、SCN-08、SCN-09、SCN-10、SCN-16。
- Diagnosing Stage 5：把最小红灯保留为正确命令/manager seam 的回归测试，观察 RED → GREEN，并重跑原始反馈环。

## Acceptance Criteria

- [ ] disabled Managed 主引用在保存后立即更新 model family、Base URL、MCP 与模型目录，无需切换 provider。
- [ ] 即使本次只改 Base URL，也重新应用供应商有效 model family。
- [ ] enabled Managed 主引用保持 listener 严格字段和运行时不中断，同时 Home、catalog 与新请求 runtime 使用新配置。
- [ ] 在途请求继续使用进入时快照；保存后新请求使用新快照。
- [ ] enabled 保存后关闭路由，新 model family 与所有 Profile 扩展保留。
- [ ] 多 Managed 主引用全部同步；External/failover-only Home 不变，运行中 failover snapshot 按既有合同更新。
- [ ] 任一受影响 Profile 失败时，Home/catalog/route backup/DB 按现有全引用合同补偿。
- [ ] 错误不泄露 token、API Key 或配置正文，provider key rename 仍拒绝。
- [ ] 诊断测试 `shared_provider_save_updates_disabled_primary_profile_model` 从 RED 变 GREEN，原始反馈环不再复现。

## 验证方式

- 运行 Provider 命令核心的精确回归测试。
- 运行 Route Manager 的 disabled、enabled、mixed reference、runtime hot swap、rollback 与 disable 测试。
- 运行原始反馈环命令并记录 GREEN 输出。
- 运行 Rust formatter、静态检查与相关 crate 测试。

## 执行约束

- 修改测试或生产代码前调用 `$tdd`；每个行为用独立 tracer bullet 推进。
- 复用 Issue 01 的 model-family 语义，不在 orchestration 中重复 TOML 合并规则。
- enabled 计划必须把 routed Home、catalog 与 route backup metadata 纳入现有补偿边界，不改变备份 schema。
- 不改变普通 startup、External、failover-only Home 与在途请求合同。

## 范围之外

- 手动 Sync、Import/Restore best-effort 扇出与 UI warning。

## Comments

- 当前诊断测试已存在且稳定 RED；worker 应先验证 RED，不得先修改生产实现。
- RED/GREEN：诊断测试 `shared_provider_save_updates_disabled_primary_profile_model` 先稳定失败于 Home 仍为 `old-model`，新增显式直连权威模型族计划并接入保存编排后转绿；enabled tracer `shared_provider_save_hot_swaps_enabled_primary_runtime_without_restarting` 先失败于 routed Home 缺少 `new-default-model`，将模型族与目录从同一 Home 快照组合进可补偿计划后转绿。
- 行为覆盖：最终 `shared_provider_save_*` 目标集 15/15 通过，覆盖 disabled/ enabled Managed 主引用、Common Config 最终优先、model/reasoning 覆盖与删除、MCP/目录/Profile 扩展保留、listener/runtime/in-flight、disable 后终态、多主引用、External、failover-only、批次与数据库提交补偿、CAS 冲突及敏感信息保护；provider key rename 拒绝回归 1/1 通过。
- 静态验证：`cargo fmt --check` 与 `cargo check --all-targets` 通过。严格 Clippy 被 10 个既有无关 lint 阻断（`migration.rs` 1、`body_dump.rs` 6、`forwarder.rs` 1、`route_manager.rs:12705` 1、`prompt_files.rs` 1），本 issue 新增/修改代码未产生 Clippy 报告。
- 全量说明：本 issue 已完成指定目标集；完整 Rust 测试套件留给 Issue 04 最终一致性审计执行。
