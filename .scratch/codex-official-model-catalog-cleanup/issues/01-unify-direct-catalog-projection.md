# 01 — 统一直连切换的模型目录投影

Status: resolved

**构建内容：** 关闭态 Managed Profile 通过真实 provider 切换入口选择无模型目录的官方或第三方主供应商后，cc-switch 自有目录指针会被清理；选择带目录的供应商时，目标 Home 会得到正确目录文件和相对指针。该行为由共享 direct plan 提供，而不是由 official 特判实现。

**受阻于：** 无——可以立即开始。

## 覆盖范围

- Requirements：REQ-01、REQ-05、REQ-06、REQ-07、REQ-09、REQ-10。
- Scenarios：SCN-01、SCN-02、SCN-03、SCN-04、SCN-08、SCN-09。
- Test seams：TS-01、TS-02。

## Acceptance Criteria

- [x] 诊断阶段的关闭态官方切换 regression test 在生产修复前稳定失败，修复后通过，且仍同时证明主供应商引用已更新、最终配置已清理指针。
- [x] 非 official 的无目录供应商遵循相同清理行为，不依赖 category 分支。
- [x] direct plan 对带有效目录的供应商仍在目标 Profile Home 写入相对指针和目录文件，不污染其他 Home。
- [x] 无目录供应商只清理 cc-switch 自有指针；用户自定义目录指针、Profile 自有扩展和旧目录文件保持不变。
- [x] authoritative 与 automatic direct plan 都从同一原始 Home 快照组合目录投影和 provider 投影，最终配置只对应一个 target。
- [x] 计划构造后的外部 Home 编辑会触发现有冲突保护，不被陈旧计划覆盖；辅助文件不留下不安全的部分写入。

## 验证方式

- 先运行 `cargo test --lib switching_disabled_profile_to_official_clears_catalog_pointer -- --nocapture` 并记录 red，再在修复后运行同一命令并记录 green。
- 运行 direct plan、空目录 ownership、用户自定义 pointer、旧目录文件和 external config change 相关的 focused tests。
- 运行 `cargo fmt --check` 与 `git diff --check`。

## 执行约束

- 修改任何测试或生产代码前调用 `$tdd`，一次完成一个 red→green vertical slice。
- 通过 Home Config Service 的公开 plan seam 验证外部行为，不测试 private helper 或内部调用次数。
- 镜像 routed plan 的单快照组合语义；不得在 Route Manager 串联两个独立配置写入。
- 继续复用现有 ownership helper、配置指纹、目录文件计划和恢复机制。
- 不增加 official 特判，不删除用户自定义 pointer，不删除旧目录文件。
- 保留全部既有注释；新增注释使用中文；新增函数遵循单一职责和项目常量约束。
- 完成并验证后只 stage 本 issue 的修改，提交一次 issue-scoped commit，不 push；commit message 要体现被证实的根因是“目录删除作用于 provider config 而非当前 Home”。

## 范围之外

- 明确 Sync、Import/Restore、共享供应商保存和启动对账的高层 lifecycle 回归。
- 旧 ProviderService 对照路径的扩展。
- 更改 catalog ownership 或删除孤立目录文件。

## Comments

- 诊断阶段已存在一个未提交的最小 red test；它属于本 issue，应由 worker 保留、验证并随本 issue 提交。
- Review Base `d6e13d43b64a16930dfaed9cda4890dd05b192e2` 是执行时 HEAD 的祖先，本 issue 无 blocker。
- Red 证据：`cargo test --lib switching_disabled_profile_to_official_clears_catalog_pointer -- --nocapture` 实际运行 1 个测试并失败；route 已更新，但最终 Home 仍包含 `model_catalog_json = "cc-switch-model-catalog.json"`。
- Green 证据：在 shared direct plan 内基于同一 Home 快照先完成 ownership-aware 目录指针投影，再合并 provider 投影；同一 focused test 结果为 1 passed。
- 回归证据：关闭态 official 与非 official 无目录切换 2 passed；direct-provider focused 测试 7 passed；Home Config Service 模块 47 passed；底层空目录 ownership 测试 2 passed。
- 覆盖边界：authoritative/automatic 两种模式、用户自定义 pointer、Profile 扩展、旧目录文件、目标 Home 隔离、外部并发编辑与辅助文件补偿、恢复到投影前原始快照。
- 静态验证：`cargo fmt --all --check` 与 `git diff --check` 均通过。
- Review follow-up：Code review 指出 direct builder 重复了从 Home snapshot 解码并准备 catalog projection 的编排，还使低层 setter 扩大到 `pub(crate)`。已提取私有 `prepare_model_catalog_projection_from_snapshot` 供 catalog plan 与 direct plan 共用，并恢复 setter 私有可见性。
- Review follow-up 验证：重构前原始 public-seam 回归 1 passed、direct-provider 7 passed；重构后同样为 1 passed 与 7 passed，`catalog` 集合 75 passed。`cargo fmt --all --check` 与 `git diff --check` 通过，`cargo clippy --lib` 通过且仅报告未修改文件中的 8 个既有 warning。
