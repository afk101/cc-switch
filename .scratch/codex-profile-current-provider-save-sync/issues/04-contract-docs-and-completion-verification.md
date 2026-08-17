# 04 — 合同文档与完成验证封口

Status: resolved

**构建内容：** 用最终实现和测试结果固化 Codex Profile model-family 所有权、启动例外、原子保存与 best-effort 同步合同，并完成 Bug 的全量回归和清理审计。

**受阻于：** 02 — Provider 保存同步全部 Managed 主引用；03 — 多 Profile 显式 Sync、Import 与云恢复。

## 覆盖范围

- Requirements：REQ-01、REQ-02、REQ-03、REQ-04、REQ-05、REQ-06、REQ-07、REQ-08、REQ-09、REQ-10、REQ-11、REQ-12、REQ-13、REQ-14、REQ-15、REQ-16、REQ-17。
- Scenarios：SCN-01、SCN-02、SCN-03、SCN-04、SCN-05、SCN-06、SCN-07、SCN-08、SCN-09、SCN-10、SCN-11、SCN-12、SCN-13、SCN-14、SCN-15、SCN-16、SCN-17、SCN-18。
- Diagnosing Stage 6：原始 repro、回归测试、调试标记、throwaway artifact、最终 hypothesis 与全量验证全部封口。

## Acceptance Criteria

- [x] 领域文档和 ADR 明确：Provider effective model family 权威、Profile 扩展保留、普通 startup 保留 model family。
- [x] 文档区分交互式 provider save 的原子合同与 Sync/Import/Restore 的 best-effort 合同。
- [x] 文档说明无 journal 的崩溃残留由下一次明确动作收敛。
- [x] 原始反馈环和所有新增回归测试通过，Base URL、MCP、catalog、runtime、External、failover、startup、rollback 既有测试无回退。
- [x] Rust、前端 typecheck/测试、格式与静态检查按仓库能力完成；任何环境或既有失败均有归因证据。
- [x] 仓库不存在本任务添加的 `[DEBUG-...]` instrumentation 或 throwaway prototype。
- [x] 最终 issue commit/总结说明被证实的 hypothesis：自动投影无条件保留 model family 导致保存成功但 Home 保留旧值。
- [x] Findings 的 requirement/scenario 覆盖矩阵与完成审计更新为最终证据。

## 验证方式

- 重跑诊断阶段精确命令与完整相关 Rust 测试集合。
- 运行完整 Rust test suite、前端 test/typecheck/build、formatter 与 Clippy；使用系统 Clang 环境。
- 搜索 `[DEBUG-`、临时 harness 和未处理的本任务 TODO。
- 对照 spec 逐项审计 REQ/SCN 的测试或可观察证据。

## 执行约束

- 修改代码或测试前调用 `$tdd`；纯文档同步必须以已通过的最终行为为准。
- 不用文档掩盖未通过的 requirement；缺证据即继续修复或补测试。
- 不删除既有注释或解释性备注。
- 本 issue 完成后提交一次 issue-scoped commit，不 push。

## 范围之外

- 新的架构改造建议；Bug 完成后由 coordinator 单独询问如何防止再次发生。

## Comments

- Review 与 docs-spec 同步由 `$implement` coordinator 在所有 issues 完成后按 skill 门禁执行。
- 领域合同：`CONTEXT.md` 新增有效模型族、Profile 自有扩展与明确 Profile 同步词汇；ADR-0001 已明确 effective Provider + Common Config 的字段级权威、External/failover 边界、startup 例外、provider save 原子补偿、Sync/Import/Restore best-effort warning 以及无 journal 的下一次明确动作收敛。
- Stage 6：原始反馈环已 GREEN；最终 hypothesis 确认为自动投影的 `PreserveUserModel` 无条件保留 model family，导致数据库保存成功而 Home 仍为旧值。Base URL、MCP、catalog、runtime/in-flight、External、failover、startup、CAS 与 rollback 回归均通过，未发现 `[DEBUG-...]` 或 throwaway artifact。
- 完成审计：findings 已逐项列出 REQ-01～REQ-17 与 SCN-01～SCN-18 的公开 seam 证据，全部结论为已证实；没有用窄测试替代宽合同。
- Rust：原始反馈环 1/1、model-family 3/3、provider save 15/15、explicit sync 3/3、startup 2/2、import/export 26/26 通过；全量 lib 2651 passed/5 ignored，其余集成目标通过。既有 `provider_commands` 有两个不带 Profile 的旧 Codex switch 测试与当前合同失配，另两个失败仅为 mutex poison 连带；Review Base 已含失配，隔离运行证实无关用例通过。fmt、check、非严格 Clippy 通过；严格 Clippy 仅报告 10 个既有无关 lint。
- 前端：756 个 Vitest 测试通过；全量命令唯一失败为 Vitest 错收 Node TAP 文件，正确 `node --test scripts/upgrade.test.js` 1/1 通过。typecheck、Prettier check 与 renderer build 通过。
