# 02 — Provider 保存同步全部 Managed 主引用

Status: ready-for-agent

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
