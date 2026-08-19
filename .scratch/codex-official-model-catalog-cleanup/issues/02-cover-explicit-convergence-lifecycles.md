# 02 — 覆盖明确同步与供应商保存的目录收敛

Status: ready-for-agent

**构建内容：** 当关闭态 Managed Profile 的当前主供应商不再提供模型目录时，手动 Sync、数据库导入或云恢复使用的后置同步，以及共享供应商保存，都会清理旧的 cc-switch 目录指针，并继续遵守各自既有的 best-effort 或整体补偿语义。

**受阻于：** 01 — 统一直连切换的模型目录投影。

## 覆盖范围

- Requirements：REQ-02、REQ-03、REQ-05、REQ-06、REQ-07、REQ-08、REQ-09、REQ-10。
- Scenarios：SCN-03、SCN-04、SCN-05、SCN-06、SCN-08、SCN-10、SCN-11。
- Test seams：TS-03、TS-04。

## Acceptance Criteria

- [ ] 明确 Profile Sync 对无目录主供应商清理关闭态 Managed Home 的 cc-switch 指针，并保留 Profile 自有扩展和既有 best-effort warning 语义。
- [ ] Import/Restore 继续复用同一明确同步 seam，不新增独立目录清理分支。
- [ ] 共享供应商保存从有目录变为无目录时，所有关闭态 Managed 主引用 Home 在保存成功前解除 cc-switch 指针。
- [ ] 共享供应商保存仍跳过 External Home，故障转移引用仍不决定 Home 模型目录。
- [ ] 批量 Home 写入或供应商数据库提交失败时，已经变化的 Home 按现有全量补偿语义恢复，错误信息不泄露敏感配置正文。
- [ ] 目标供应商仍有有效目录时，明确同步和供应商保存继续更新正确目标 Home 的目录内容。

## 验证方式

- 逐个添加并运行显式同步无目录 provider、共享供应商保存删除 `modelCatalog` 的 focused tests；每个新行为先 red 后 green。
- 运行现有 explicit sync、shared provider save、External Home、批量补偿和并发拒绝相关测试。
- 运行 Route Manager 相关 lib tests、`cargo fmt --check` 与 `git diff --check`。

## 执行约束

- 修改任何测试或生产代码前调用 `$tdd`；使用 Route Manager 公开同步和共享供应商更新 seam，不绕过接口直接断言内部调用。
- 不为 Sync、Import、Restore 或 provider save 复制 catalog 清理逻辑；它们必须消费 issue 01 统一后的 direct plan contract。
- 不改变明确同步的有效模型族权威性、逐 Profile best-effort warning 或共享供应商保存的整体补偿语义。
- 保留全部既有注释；新增注释使用中文；新增函数遵循单一职责和项目常量约束。
- 完成并验证后只 stage 本 issue 的修改，提交一次 issue-scoped commit，不 push。

## 范围之外

- 启动 automatic reconcile 的 model-family 保留行为。
- UI 点击级自动化。
- External Home ownership 迁移。

## Comments

- 数据库导入与云恢复通过现有后置明确同步入口获得覆盖；若 code audit 发现它们绕过该 seam，必须停止并更新 findings，而不是自行扩展架构。
