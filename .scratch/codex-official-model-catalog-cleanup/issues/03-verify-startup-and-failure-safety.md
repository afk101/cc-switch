# 03 — 验证启动对账与失败安全

Status: ready-for-agent

**构建内容：** 应用启动时，关闭态 Managed Profile 会按数据库当前主供应商修复旧的 cc-switch 模型目录指针，同时保留用户当前 model family；显式往返、并发修改和持久化失败场景均不会留下错误或不可恢复状态。

**受阻于：** 01 — 统一直连切换的模型目录投影。

## 覆盖范围

- Requirements：REQ-01、REQ-04、REQ-06、REQ-07、REQ-08、REQ-09、REQ-10。
- Scenarios：SCN-03、SCN-07、SCN-08、SCN-09、SCN-10、SCN-11、SCN-12。
- Test seams：TS-01、TS-02、TS-05。
- Diagnosing Bug contract：Stage 5 原始场景复验；Stage 6 regression、清理与 post-mortem 检查。

## Acceptance Criteria

- [ ] 启动对账清理无目录当前主供应商对应的旧 cc-switch 指针，同时逐字保留用户当前顶层 model family 和 Profile 自有扩展。
- [ ] 启动对账继续跳过 External Home，并在单 Profile 失败时隔离错误、继续处理其他 Profile。
- [ ] 显式切换在 Home 应用成功但数据库持久化失败时，恢复切换前包含旧指针的完整 Home；并发外部编辑不会被补偿覆盖。
- [ ] “带目录第三方 → 无目录官方”的完整关闭态 Profile 往返路径通过，第三方阶段存在相对指针，官方阶段指针消失。
- [ ] 诊断时添加在旧 ProviderService seam 的绿色对照和扩展断言被删除，不以错误调用链测试充当验收证据。
- [ ] 不存在 `[DEBUG-...]` 临时 instrumentation，不存在 throwaway prototype，所有相关 regression tests、格式检查和差异检查通过。

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
