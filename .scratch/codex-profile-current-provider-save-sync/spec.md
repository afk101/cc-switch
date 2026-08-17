# Codex Profile 供应商模型配置权威同步

## 问题陈述

用户在 CC Switch 中编辑当前 Codex 供应商的默认模型或 Base URL 并点击保存后，数据库里的供应商已经更新，但受该供应商管理的 Profile Home 可能仍保留旧 `model`。用户必须切换到其他供应商再切回来，新的默认模型才会写入对应 `CODEX_HOME/config.toml`。这让“保存成功”与实际生效配置不一致。

根因是 Fork 为保护用户在 Home 中临时选择的模型，将自动供应商投影改为无条件保留顶层 `model` 与全部 `model_reasoning_*`。该策略违背上游原版“当前供应商有效配置对 Live 模型配置权威”的行为，并且没有覆盖多 Profile、路由开启态与显式同步操作。

## 目标

- 用户保存当前主供应商后，所有受管理 Profile 立即使用该供应商的有效模型配置，无需切走再切回。
- 将上游原版的供应商权威语义适配到多 Profile：供应商管理的 model family 覆盖或删除，Profile 自有扩展保留。
- 路由开启和关闭状态表现一致，同时保持 listener、runtime、模型目录、在途请求和关闭恢复合同。
- 手动 Sync、数据库导入与云恢复能够按每个 Managed Profile 的主供应商显式同步 model family，并准确报告部分失败。
- 保持交互式供应商保存现有的全引用原子补偿、安全冲突检测和敏感信息保护。

## 非目标

- 不让普通应用启动或启动对账重置 Home 当前 model family。
- 不修改 External Profile Home。
- 不让仅作为故障转移候选的供应商覆盖 Profile Home model family。
- 不支持 Codex provider key 重命名；该能力属于独立功能。
- 不取消、迁移或重写已经进入转发过程的在途请求。
- 不用供应商整文件替换 Profile Home，不删除 Profile 独有 Desktop、插件和未知扩展。
- 不新增跨数据库与多个 Home 文件的持久化事务日志。

## 解决方案

把供应商保存理解为一次明确的“重新应用供应商有效配置”操作。有效配置由供应商存储配置与 Common Config 合并而成，冲突时 Common Config 保持现有最终优先级。对于每个以该供应商为主供应商的 Managed Profile，`model` 与全部顶层 `model_reasoning_*` 按有效配置权威覆盖；有效配置未声明或声明空模型时，删除 Home 中的旧字段。

投影只接管 provider 受管字段。每个 Profile 独有的 Desktop、插件和未知扩展继续保留。路由开启态在当前 routed Home 上定点投影 model family，并继续保持本地 listener 路由字段、模型目录和 runtime 热更新；路由关闭后，新 model family 与 Profile 扩展继续存在。

普通启动不执行 model-family 权威投影。保存、切换、手动 Sync、数据库导入和云恢复属于明确同步动作。交互式供应商保存保持全引用原子性；Sync、Import/Restore 采用逐 Profile best-effort，并通过脱敏警告呈现部分失败。

## 用户故事

1. 作为使用自定义 `CODEX_HOME` 的用户，我想在供应商编辑页保存新模型后立即看到对应 `config.toml` 更新，从而不需要切换供应商触发刷新。
2. 作为同时管理多个 Codex Profile 的用户，我想同一主供应商的所有 Managed Profile 一次同步，从而避免不同 Profile 使用不同版本的模型配置。
3. 作为开启本地路由的用户，我想保存供应商后新请求立即采用新模型，同时不重启 listener 或中断在途请求。
4. 作为在 Profile 中配置插件和 Desktop 选项的用户，我想供应商同步只更新供应商负责的字段，从而不丢失每个 Profile 的个性化配置。
5. 作为维护 Common Config 的用户，我想继续沿用 Common Config 的现有优先级，从而不因本修复改变有效配置合并结果。
6. 作为清除供应商默认模型的用户，我想保存后所有 Managed Profile 删除旧模型，从而不会继续使用已经撤销的默认值。
7. 作为使用 Official Provider 的用户，我想未声明模型保持合法，并清除此前供应商遗留的模型字段。
8. 作为执行手动 Sync 的用户，我想每个 Managed Profile 按自己的主供应商刷新，从而让多 Profile 实际状态与数据库一致。
9. 作为执行数据库导入或云恢复的用户，我想恢复完成后立即同步所有 Managed Profile，从而不出现数据库已恢复但 Home 永久停留旧配置的情况。
10. 作为遇到个别不可写 Home 的用户，我想其他 Profile 继续同步，并清楚看到失败名单，从而能修复权限后重试。
11. 作为直接在 Codex Home 临时选模的用户，我想普通启动 CC Switch 时不被重置，从而只有明确同步动作才重新应用供应商模型。
12. 作为安全敏感用户，我想错误信息不泄露 token、API Key 或配置正文。
13. 作为遭遇并发外部文件修改的用户，我想 CC Switch 拒绝覆盖冲突并按既有合同补偿，从而避免静默丢失外部修改。

## 可观察 Requirements

- **REQ-01**：保存 Codex 供应商成功返回前，所有以该供应商为主供应商且 Home ownership 为 Managed 的 Profile 必须应用其有效 model family；enabled 与 disabled 均适用。
- **REQ-02**：有效 model family 包含顶层 `model` 与所有名称以 `model_reasoning_` 开头的顶层字段。
- **REQ-03**：有效配置中的 model-family 值必须覆盖 Home 旧值；有效配置未声明相应字段时必须删除 Home 旧值；空字符串 `model` 必须归一化为未声明而不是写入空模型。
- **REQ-04**：有效配置继续使用现有 Provider + Common Config 合并合同，标量冲突时 Common Config 最终优先。
- **REQ-05**：model-family 投影不得删除或覆盖 Profile 独有的 Desktop、插件、未知扩展及不属于本次投影的用户字段。
- **REQ-06**：disabled Managed Profile 在同步后保持完整直连供应商配置、数据库管理的 MCP 与模型目录。
- **REQ-07**：enabled Managed Profile 在同步后保持本地 listener 的 Base URL、wire API 与凭证，更新模型目录和 runtime snapshot，不重启 listener；新请求采用新配置，在途请求保持进入时快照。
- **REQ-08**：enabled Profile 后续关闭路由时，只恢复严格路由字段；同步后的 model family 与 Profile 扩展继续保留。
- **REQ-09**：External Profile 不参与任何自动或显式 Profile Home 投影；仅故障转移引用不得改变 Home 或模型目录，运行中的引用仅更新 runtime snapshot。
- **REQ-10**：普通应用启动和启动对账必须保留 Home 当前 model family，不因数据库供应商值重置。
- **REQ-11**：手动 Sync、数据库导入和云恢复必须遍历全部 Managed Profile，并按每个 Profile 自己的主供应商应用有效 model family。
- **REQ-12**：手动 Sync、Import/Restore 对 Profile 同步采用 best-effort；单个失败不回滚已成功 Profile，也不阻止后续 Profile。
- **REQ-13**：best-effort 部分失败必须返回“完成但有警告”的可观察结果，列出失败 Profile 与脱敏原因；Import/Restore 的数据库成功状态保持成功。
- **REQ-14**：交互式供应商保存继续采用全引用成功或整体补偿；任一受影响 Profile 失败时，不提交新供应商数据库状态，并恢复已应用 Home/目录/路由备份副作用。
- **REQ-15**：Home 指纹冲突继续拒绝强制覆盖；错误和补偿信息不得包含 token、API Key 或配置正文。
- **REQ-16**：进程崩溃或 best-effort 残留不引入 journal，也不由普通启动重置 model family；下一次明确 Save/Switch/Sync/Import/Restore 可以继续收敛。
- **REQ-17**：Codex provider key rename 继续拒绝，不在本功能中迁移 Profile 引用。

## Scenarios

- **SCN-01 — Disabled 当前供应商保存模型**：Given 一个 Managed/disabled Profile 的 Home 为 `old-model`，且其主供应商待保存为 `new-model`；When 用户保存供应商；Then 数据库和 Home 均为 `new-model`，无需切换供应商。
- **SCN-02 — 无关字段保存仍重新应用默认模型**：Given Home 被临时改为其他模型，供应商默认模型未改变；When 用户只修改 Base URL 并保存供应商；Then Home 仍重新应用供应商有效默认模型，Base URL 同时更新。
- **SCN-03 — 删除 model family**：Given Home 存在旧 `model` 与多个 `model_reasoning_*`；When 有效供应商配置不再声明这些字段或 `model` 为空；Then对应 Home 字段被删除，其他扩展保留。
- **SCN-04 — Common Config 冲突**：Given Provider 与 Common Config 声明不同 model-family 值；When 执行明确同步；Then Home 使用 Common Config 的最终值。
- **SCN-05 — 保留 Profile 扩展**：Given Home 含 Desktop、插件与未知扩展；When model family 被覆盖或删除；Then扩展内容逐字义保持，只有受管字段变化。
- **SCN-06 — Enabled 保存**：Given Managed/enabled Profile 正通过本地 listener 路由；When 保存主供应商；Then routed Home 使用新 model family，本地路由严格字段保持，catalog/runtime 更新且 listener 不重启。
- **SCN-07 — 在途请求隔离**：Given 保存发生时存在已进入路由的请求；When provider runtime 热替换；Then在途请求完成于旧快照，保存后的新请求使用新快照和新 Home 模型。
- **SCN-08 — 保存后关闭路由**：Given enabled Profile 已同步新 model family；When 用户关闭路由；Then上游直连严格字段恢复，新 model family、Desktop、插件、MCP 与未知扩展保留。
- **SCN-09 — 多引用扇出**：Given 多个 enabled/disabled Managed Profile 以同一 provider 为主引用，另有 External 与 failover-only 引用；When 保存该 provider；Then全部 Managed 主引用同步，External/failover-only Home 不变，运行中的 failover runtime 更新。
- **SCN-10 — 保存批次失败**：Given 多个受影响 Profile 中一个 Home 不可写；When 保存 provider；Then保存失败、数据库保持旧 provider、已应用 Profile 与相关目录/备份恢复，无敏感内容出现在错误中。
- **SCN-11 — 启动保留临时选模**：Given 用户在 Home 临时修改 model family；When普通启动或启动对账；Then model family 保持，其他既有派生状态仍按原合同对账。
- **SCN-12 — 手动 Sync 多 Profile**：Given 不同 Managed Profile 使用不同主供应商；When用户手动 Sync；Then每个 Profile 按自己的有效主供应商同步，External 跳过。
- **SCN-13 — Import/Restore 全成功**：Given数据库导入或云恢复成功且所有 Home 可写；When post-import sync 执行；Then全部 Managed Profile 收敛，操作报告成功。
- **SCN-14 — Import/Restore 部分失败**：Given数据库恢复成功但部分 Home 不可写；When post-import sync 执行；Then成功 Profile 保持新状态、失败 Profile 保持可恢复状态、后续 Profile继续处理，UI 报告数据库成功并附脱敏警告。
- **SCN-15 — 手动 Sync 部分失败**：Given某些 Managed Home 写冲突或不可写；When手动 Sync；Then其余 Profile继续同步，结果为完成但有逐 Profile 警告。
- **SCN-16 — 并发外部修改**：Given计划构造后 Home 被外部修改；When应用投影；Then该 Profile 报冲突且不被强行覆盖，所属操作按交互式原子或 best-effort 合同处理。
- **SCN-17 — Official Provider 无模型**：Given Official Provider 未声明 model family，Home 残留旧自定义模型；When执行明确同步；Then旧 model family 删除，官方空配置保持合法。
- **SCN-18 — 崩溃残留**：Given明确同步在部分副作用后进程退出；When下一次普通启动；Then启动不重置 model family；When用户随后执行明确同步；Then可重新收敛。

## 关键决策

- 每次明确供应商保存都重新应用默认 model family，不以“本次模型是否变化”为条件。
- model 与全部 `model_reasoning_*` 采用上游原版的供应商权威语义；缺失同样是权威删除。
- 多 Profile 采用字段所有权适配，不采用整文件覆盖。
- enabled 与 disabled 行为一致；External 与 failover-only Home 不进入主供应商投影。
- Common Config 保持现有最终优先级。
- 普通启动不属于 model-family 明确同步动作。
- Save 保持原子；Sync/Import/Restore 选择 best-effort 与部分成功警告。
- 不用 journal 修复崩溃残留，由下一次明确动作收敛。

## 实施决策

- 建立一个纯 model-family 投影能力：以当前 Home 与有效供应商配置为输入，仅权威同步 model family，并保留非受管字段。
- disabled 直连投影与 enabled routed 投影复用同一 model-family 语义；enabled 通过当前 routed Home 变换后重新施加 listener 严格字段。
- enabled 的 provider-save 计划需要把 model-family Home 变更、模型目录、路由备份元数据与 runtime 更新纳入同一现有补偿边界。
- 显式 Sync 建立 route-aware 高层接口，按每个 Profile 的主供应商构造有效配置；现有命令与 Import/Restore post-sync 只做薄委托。
- best-effort 结果使用结构化逐 Profile outcome，UI/命令层负责呈现成功计数与脱敏警告。
- 不改变数据库 schema、路由备份版本或 provider key 合同。

## 错误行为与恢复

- Provider save 任一 Profile 准备或应用失败时，返回失败并恢复已发生的 Home、catalog、route backup 与数据库副作用；补偿失败沿用现有“未收敛”诊断。
- Sync/Import/Restore 捕获单 Profile 失败并继续，最后返回成功结果与 warnings。warnings 只包含 Profile 标识、名称、Home 路径及公开错误摘要。
- Import/Restore 的 DB 一旦成功不因 Profile 同步失败回滚。
- 指纹冲突不重试覆盖，用户修复冲突或权限后可再次执行明确同步。
- 崩溃不引入新恢复日志；普通启动不把 DB model family强行写回 Home。

## 兼容性

- 当前 Fork 发布基线为上游 v3.19.2；恢复其 Provider 对 Live model/reasoning 权威的核心语义。
- 保留现有 Common Config、MCP、模型目录、runtime 热切换、listener 与私有身份文件合同。
- Official 与 Custom Provider 都允许无 model；无 model 统一解释为删除 Home 旧 model。
- 既有 External、failover-only、CAS、补偿和 provider rename 行为保持。

## 测试决策

- 采用 TDD：每个 vertical slice 先在确认 seam 写可因该行为变红的测试，再做最小实现。
- 回归测试验证 public behavior：保存/同步操作完成后读取临时 Home、数据库、runtime 状态与公开结果，不断言 private helper 调用。
- 保留诊断阶段的最小红灯用例，并在修复后重新运行原始反馈环。
- enabled 测试覆盖 route 字段、catalog、runtime、不重启、在途请求与 disable 后终态。
- 错误测试覆盖交互式原子补偿和 best-effort 聚合警告两种不同合同。
- 最终运行目标测试、Rust 格式/Clippy、前端 typecheck/相关测试和完整测试套件；环境必须显式使用系统 Clang，避免 NVM 的同名 `cc` 劫持。

## Test Seams

- **Provider 命令核心 seam**：真实内存数据库、临时 Profile Home 与供应商保存核心入口；覆盖用户报告的 disabled 保存症状、Common Config、数据库提交与 Home 终态。
- **Codex Route Manager seam**：覆盖 enabled/disabled/mixed references、catalog/runtime、在途请求、补偿与 disable 后终态。
- **Codex Home 配置服务 seam**：覆盖 model-family 纯投影的覆盖、删除、空值与扩展保留，不测试内部递归实现。
- **Route-aware explicit sync seam**：覆盖多 Profile 各自主供应商、External、best-effort outcomes；Sync/Import/Restore 命令测试只验证委托与用户可见结果。

## 范围之外

- Provider key rename 与引用迁移。
- External Home 重新接管流程。
- 启动时 model-family 全量收敛。
- 持久化跨文件 transaction journal。
- 重新设计 Common Config 或模型目录格式。

## 补充说明

- 已确认根因提交为 Fork 的 `29768fe5`：自动投影无条件跳过 `model` 与 `model_reasoning_*`。
- 原始反馈环为精确 Rust 测试 `shared_provider_save_updates_disabled_primary_profile_model`；修复前数据库新模型断言通过，Home 仍为旧模型。
- Base URL 高层回归在诊断阶段保持绿灯，说明本修复应扩展 model-family 权威字段而不回退既有 provider 字段同步。
