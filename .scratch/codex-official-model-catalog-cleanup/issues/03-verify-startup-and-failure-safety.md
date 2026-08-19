# 03 — 验证启动对账与失败安全

Status: resolved

**构建内容：** 应用启动时，关闭态 Managed Profile 会按数据库当前主供应商修复旧的 cc-switch 模型目录指针，同时保留用户当前 model family；显式往返、并发修改和持久化失败场景均不会留下错误或不可恢复状态。

**受阻于：** 01 — 统一直连切换的模型目录投影。

## 覆盖范围

- Requirements：REQ-01、REQ-04、REQ-06、REQ-07、REQ-08、REQ-09、REQ-10。
- Scenarios：SCN-03、SCN-07、SCN-08、SCN-09、SCN-10、SCN-11、SCN-12。
- Test seams：TS-01、TS-02、TS-05。
- Diagnosing Bug contract：Stage 5 原始场景复验；Stage 6 regression、清理与 post-mortem 检查。

## Acceptance Criteria

- [x] 启动对账清理无目录当前主供应商对应的旧 cc-switch 指针，同时逐字保留用户当前顶层 model family 和 Profile 自有扩展。
- [x] 启动对账继续跳过 External Home，并在单 Profile 失败时隔离错误、继续处理其他 Profile。
- [x] 显式切换在 Home 应用成功但数据库持久化失败时，恢复切换前包含旧指针的完整 Home；并发外部编辑不会被补偿覆盖。
- [x] “带目录第三方 → 无目录官方”的完整关闭态 Profile 往返路径通过，第三方阶段存在相对指针，官方阶段指针消失。
- [x] 诊断时添加在旧 ProviderService seam 的绿色对照和扩展断言被删除，不以错误调用链测试充当验收证据。
- [x] 不存在 `[DEBUG-...]` 临时 instrumentation，不存在 throwaway prototype，所有相关 regression tests、格式检查和差异检查通过。

## 验证方式

- 逐个添加并运行启动对账清理、持久化失败恢复和完整 Profile 往返的 focused tests；每个新增行为先 red 后 green。
- 重跑 issue 01 的最小 regression test、issue 02 的 lifecycle tests，以及 Route Manager/Home Config/Codex Config 相关测试集合。
- 搜索 `[DEBUG-` 和诊断 test 名称，确认没有临时 instrumentation 或旧 ProviderService throwaway test。
- 运行 `cargo fmt --check`、`git diff --check` 和项目可承受范围内的完整 Rust test suite；任何环境或 baseline 失败必须逐项分类并记录。

## 执行约束

- 修改任何测试或生产代码前调用 `$tdd`；通过公开启动对账、切换与 direct plan seams 验证行为。
- automatic 模式只能改变供应商派生目录状态，不得权威覆盖用户当前 model family。
- 不把补偿式原子性描述为跨文件 OS transaction；继续使用现有 fingerprint 与 conditional restore 保护外部编辑。
- 保留全部既有注释；新增注释使用中文；新增函数遵循单一职责和项目常量约束。
- 完成并验证后只 stage 本 issue 的修改，提交一次 issue-scoped commit，不 push；commit message 或提交正文应说明最终证实的 hypothesis。

## 范围之外

- 为两个文件增加持久化事务日志。
- 删除旧目录文件或改变 ownership 规则。
- 已启用 routed Profile 的模型目录实现改造。

## Comments

- 本 issue 负责 diagnosing Stage 6 的 implementation cleanup；coordinator 仍需在全部 issues 完成后执行 fixed-point code review 和最终 requirement-by-requirement 审计。
- Blocker 01 已由提交 `01153d8f0a255a9283b211221983be15ea1a8d2b` 完成；findings 中唯一 Review Base 为 `d6e13d43b64a16930dfaed9cda4890dd05b192e2`，并且是当前 HEAD 的祖先。
- 启动对账测试在临时恢复 Review Base direct-plan 行为后实际运行 1 项并因 owned `model_catalog_json` 残留失败；恢复共享目录投影后同一测试通过，同时证明 `model`、两个 `model_reasoning_*`、Profile/Desktop 扩展和旧目录文件保持不变。
- 既有 `reconcile_all_profile_derived_state_repairs_disabled_homes_idempotently_and_isolates_failure` 与 `external_takeover_skips_later_disabled_derived_state_projection` 在 Route Manager 149 项回归中通过，继续证明单 Profile 失败隔离与 External Home 跳过。
- 关闭态切换数据库保存失败测试在临时移除 Home 补偿后实际运行 1 项并因 Home 停留在投影后状态失败；恢复补偿后通过，并逐字恢复包含旧目录指针的完整 Home。
- 并发恢复测试在临时移除 target fingerprint 条件检查后实际运行 1 项并因外部 Home 被原始快照覆盖而失败；恢复条件恢复后通过，外部修改逐字保留。
- 完整往返测试在临时恢复 Review Base direct-plan 行为后实际运行 1 项并因官方阶段仍保留第三方目录指针失败；恢复共享目录投影后通过，并证明主供应商引用从第三方回到官方、旧目录文件保留且 Profile 扩展不丢失。
- Stage 6 已删除 diagnosing 阶段旧 `ProviderService` 绿色对照测试及扩展断言；该文件与 HEAD 无差异。仓库搜索未发现 `[DEBUG-` 或旧对照测试名，未创建 prototype、临时 worktree 或依赖软链。
- 相关回归全部通过：issue 01/02/03 七项代表性 focused tests；Route Manager 149 项；Home Config 47 项；Codex Config 81 项；Sync Support 2 项；ProviderService 36 项。
- 完整 Rust lib 串行运行 2663 项通过、0 失败、5 ignored。完整 workspace 随后在既有 `provider_commands` 集成测试停止：两个旧测试仍调用已禁止的通用 Codex `switch_provider` 并稳定收到“必须指定 Codex Profile”，另外两个锁中毒级联项隔离运行均通过；Review Base 至当前 HEAD 及本 issue 均未修改该测试文件。
