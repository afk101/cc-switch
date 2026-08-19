# Codex 关闭态 Profile 模型目录统一收敛

## 问题陈述

当关闭态 Managed Codex Profile 从带自定义模型目录的第三方共享供应商切换到不带模型目录的官方共享供应商时，Profile 直连配置仍可能保留 `model_catalog_json = "cc-switch-model-catalog.json"`。Codex 会继续加载 cc-switch 生成的模型目录，导致用户虽然已切回官方订阅，实际看到和使用的仍可能是第三方自定义模型。

同一错误存在于所有复用关闭态 direct plan 的派生状态收敛入口，不只存在于显式切换：明确 Profile 同步、数据库导入或云恢复后的同步、共享供应商保存，以及应用启动对账均可能保留旧指针。

## 目标

- 所有关闭态 Managed Profile 的直连配置都按当前主供应商统一收敛模型目录指针。
- 目标供应商没有有效模型目录时，解除 cc-switch 自有目录指针，使官方订阅不再加载第三方模型目录。
- 目标供应商有有效模型目录时，在目标 Profile Home 中生成或更新目录文件并写入 cc-switch 相对指针。
- 保持明确同步与启动对账既有的不同 model-family 语义。
- 保持用户自定义目录指针、Profile 自有扩展、Profile 私有身份状态以及 External Home 不受影响。
- 保持现有并发冲突检测、写入补偿与数据库失败恢复语义。

## 非目标

- 不修改已启用 Profile 的 routed 配置投影；该路径已经基于当前 Home 正确组合模型目录投影。
- 不增加针对 official 类别的特殊分支；任意无有效 `modelCatalog` 的供应商都适用相同规则。
- 不删除磁盘上未再引用的 `cc-switch-model-catalog.json` 文件；本次只修复生效配置指针。
- 不更改现有 ownership 判定规则。basename 为 `cc-switch-model-catalog.json` 的指针继续视为 cc-switch 自有，其他路径继续视为用户管理。
- 不引入跨文件事务日志，也不承诺配置文件与目录文件具备操作系统级事务。
- 不改变故障转移供应商语义；故障转移引用不决定关闭态 Profile 直连配置或模型目录。

## 解决方案

关闭态 direct plan 必须从同一次 Home 快照同时推导模型目录状态和供应商直连状态，并生成唯一的最终 `config.toml` 目标：

- 先根据当前主供应商是否提供有效模型目录，在 Home 快照上增加、更新或清理 cc-switch 自有指针。
- 再在该投影结果上合入供应商直连配置。
- 明确同步模式继续权威更新有效模型族；自动启动模式继续保留用户当前 model family。
- 最终配置计划仍以原始 Home 快照为恢复基线，只进行一次配置写入。
- 有模型目录文件时继续沿用现有辅助文件写入与失败补偿；后续数据库持久化失败时继续恢复原始 Home。

## 用户故事

1. 作为使用官方 Codex 订阅的用户，我想在从第三方供应商切回官方后不再加载第三方自定义模型，从而确保实际使用的是官方模型目录。
2. 作为使用多个 Codex Profile 的用户，我想每个关闭态 Managed Profile 都按自己的主供应商收敛模型目录，从而避免不同 Home 之间相互污染。
3. 作为编辑共享供应商的用户，我想删除供应商的模型目录后所有引用它的关闭态 Managed Profile 都同步解除旧指针，从而让保存结果立即生效。
4. 作为执行手动 Sync、数据库导入或云恢复的用户，我想同步完成后 Profile 模型目录与当前主供应商一致，从而不需要额外重选供应商修复残留。
5. 作为重新启动应用的用户，我想启动对账能够修复旧的 cc-switch 目录残留，同时保留我在 Home 中手工选择的 model 与 reasoning 设置。
6. 作为维护自定义 Codex 配置的用户，我想 cc-switch 保留我指向其他文件的自定义模型目录路径，从而不覆盖非 cc-switch 所有的配置。
7. 作为维护 Profile 自有扩展的用户，我想模型目录收敛不删除 Desktop、插件或未知扩展字段，从而避免无关配置丢失。
8. 作为使用 External Home 的用户，我想任何自动或明确同步都不修改外部接管的 Home，从而保持 ownership 边界。
9. 作为依赖可靠配置切换的用户，我想外部并发编辑或数据库失败时不留下半完成状态，从而可以安全重试。

## 可观察 Requirements

- **REQ-01**：关闭态 Managed Profile 显式切换或重选到无有效模型目录的主供应商后，最终直连配置不得包含 cc-switch 自有 `model_catalog_json` 指针。
- **REQ-02**：关闭态 Managed Profile 经过手动 Sync、数据库导入或云恢复后置同步时，模型目录指针必须按该 Profile 当前主供应商增加、更新或清理。
- **REQ-03**：保存共享供应商并成功返回前，引用它的全部关闭态 Managed 主 Home 的模型目录指针必须按保存后的供应商配置收敛；任一写入或数据库提交失败时必须恢复已应用的 Home。
- **REQ-04**：应用启动对账必须按数据库当前主供应商收敛关闭态 Managed Home 的模型目录指针，同时保留 Home 当前顶层 `model` 与全部顶层 `model_reasoning_*`。
- **REQ-05**：目标供应商提供有效模型目录时，最终配置必须使用 `cc-switch-model-catalog.json` 相对指针，目录内容必须写入目标 Profile Home，不得写入其他 Home。
- **REQ-06**：目标供应商没有有效模型目录时，只能移除 cc-switch 自有指针；指向其他文件名的用户自定义 `model_catalog_json` 必须保持不变。
- **REQ-07**：解除 cc-switch 自有指针时不得删除既有目录文件，并且必须保留 Profile 自有扩展和 Profile 私有身份状态。
- **REQ-08**：External Home 与故障转移供应商引用不得因本次 direct 模型目录收敛而修改 Home 或决定目录内容。
- **REQ-09**：计划构造后的 Home 外部编辑必须触发现有并发冲突保护，不得被陈旧计划覆盖；部分写入或后续持久化失败必须按现有补偿规则恢复到安全状态。
- **REQ-10**：修复判定不得依赖供应商类别；official 和其他无有效 `modelCatalog` 的供应商必须遵循同一收敛规则。

## Scenarios

- **SCN-01 — 切回官方清理目录指针**
  - **Given**：关闭态 Managed Profile 的 Home 只含 cc-switch 自有目录指针，目标官方主供应商没有有效模型目录。
  - **When**：用户显式选择该官方供应商。
  - **Then**：主供应商引用更新成功，最终直连配置不再包含目录指针。

- **SCN-02 — 非官方无目录供应商采用同一规则**
  - **Given**：关闭态 Managed Profile 含 cc-switch 自有目录指针，目标非官方供应商没有有效模型目录。
  - **When**：用户显式选择该供应商。
  - **Then**：最终直连配置同样移除 cc-switch 自有指针。

- **SCN-03 — 用户自定义目录指针得到保留**
  - **Given**：关闭态 Managed Profile 指向文件名不是 `cc-switch-model-catalog.json` 的用户自定义模型目录，目标主供应商没有有效模型目录。
  - **When**：任一 direct 收敛入口运行。
  - **Then**：用户自定义目录指针保持不变。

- **SCN-04 — 有效目录写入目标 Home**
  - **Given**：目标主供应商提供有效模型目录，多个 Profile 拥有不同 Home。
  - **When**：目标 Profile 的 direct 收敛入口运行。
  - **Then**：目标 Home 获得相对目录指针和匹配的目录文件，其他 Home 不被写入。

- **SCN-05 — 明确同步清理旧指针**
  - **Given**：关闭态 Managed Profile 当前主供应商没有有效模型目录，Home 残留 cc-switch 自有指针。
  - **When**：手动 Sync 或 Import/Restore 后置同步运行。
  - **Then**：同步结果成功且 Home 中的旧指针被清理；其他 Profile 的单点失败仍按既有 best-effort 语义报告。

- **SCN-06 — 供应商保存删除目录**
  - **Given**：共享供应商原先带模型目录，并被一个或多个关闭态 Managed Profile 作为主供应商引用。
  - **When**：用户保存该供应商并删除其 `modelCatalog`。
  - **Then**：保存成功前所有受影响 Managed 主 Home 都解除 cc-switch 自有指针；失败时已应用 Home 与供应商数据整体补偿。

- **SCN-07 — 启动对账清理目录但保留模型族**
  - **Given**：数据库当前主供应商没有有效模型目录，关闭态 Managed Home 残留 cc-switch 指针，并包含用户当前 model family。
  - **When**：应用启动对账运行。
  - **Then**：旧指针被清理，用户当前 model family 与 Profile 自有扩展保持不变。

- **SCN-08 — 旧目录文件继续保留**
  - **Given**：Home 同时包含 cc-switch 自有指针和已生成的目录文件，目标主供应商没有有效模型目录。
  - **When**：direct 收敛成功。
  - **Then**：配置指针被移除，旧目录文件仍存在。

- **SCN-09 — 并发外部编辑受到保护**
  - **Given**：direct plan 已从 Home 快照构造，随后 Home 被外部编辑。
  - **When**：应用陈旧计划。
  - **Then**：操作返回冲突且不覆盖外部编辑，也不留下不可恢复的目录文件变更。

- **SCN-10 — 数据库提交失败恢复 Home**
  - **Given**：显式切换已将 Home 从带 cc-switch 指针的旧状态投影为无指针状态。
  - **When**：后续主供应商引用或故障转移持久化失败。
  - **Then**：Home 恢复为切换前的完整内容，数据库与 Home 不以半完成状态返回成功。

- **SCN-11 — External Home 不受影响**
  - **Given**：Profile Home 已被标记为 External，或供应商仅作为故障转移引用。
  - **When**：同步、保存或启动对账运行。
  - **Then**：该 Home 的模型目录配置不被本次规则修改。

- **SCN-12 — 原始往返路径不再复现**
  - **Given**：关闭态 Managed Profile 先使用带模型目录的第三方主供应商，再切换到无目录官方主供应商。
  - **When**：两次显式切换均成功。
  - **Then**：第三方阶段存在 cc-switch 相对指针，官方阶段该指针消失且官方主供应商引用生效。

## 关键决策

- 修复范围选择全链路统一收敛，而非只修 official 显式切换。代价是回归矩阵更大，收益是共享 direct-plan 的所有生产调用者遵循同一派生状态契约。
- 继续沿用现有 ownership 规则：只清理 cc-switch 自有文件名，保留其他用户指针。该规则避免粗暴删除用户配置，但用户手工使用相同 basename 时仍会被视为 cc-switch-owned；本次不扩大范围处理该既有边界。
- 只解除失效引用，不删除旧目录文件。孤立文件不影响 Codex，保留它可避免不必要的破坏并支持后续复用。
- 不按 `category = official` 特判；实验已经证明供应商类别不是根因。

## 实施决策

- direct plan 构造必须镜像 routed plan 的单快照组合方式：从一个原始 Home 快照构造 ownership-aware 模型目录投影，再合入模式相关的直连供应商投影。
- 最终配置计划的 previous 必须是原始 Home 快照，target 必须是组合后的唯一最终内容，避免两次可见配置写入和不完整补偿。
- authoritative 模式继续权威替换有效模型族；automatic 模式继续保留用户当前 model family。模型目录收敛独立于该模式差异。
- 继续复用现有目录文件计划、配置指纹检查、辅助文件补偿、Home 恢复和跨 Profile 批量补偿能力，不新增平行写入协议。
- 新增或调整的逻辑保持单一职责；共享目录投影不得在多个 Route Manager 调用点复制。

## 错误行为与恢复

- Home 在计划构造后发生外部编辑时，应用必须失败并保留外部内容，调用者可在重新读取权威状态后重试。
- 有效模型目录文件写入成功但最终配置写入失败时，必须按现有辅助文件补偿规则恢复目录文件。
- 显式切换完成 Home 写入后，主供应商引用或故障转移持久化失败时，必须恢复切换前 Home；恢复冲突不得覆盖后续外部编辑。
- 共享供应商保存继续采用全引用成功或整体补偿；显式 Sync/Import/Restore 继续逐 Profile best-effort，并返回既有脱敏 warning。
- 启动对账继续隔离单个 Profile 失败并记录可诊断错误，不阻断其他 Profile。

## 兼容性

- 兼容现有 Managed/External ownership、Profile 私有身份状态和故障转移引用语义。
- 兼容现有 Common Config、MCP 投影和有效模型族优先级。
- 兼容既有用户自定义模型目录路径和未知 TOML 扩展字段。
- 不要求迁移数据库或重写供应商数据；下一次显式动作或启动对账即可修复既有残留。
- 旧 cc-switch 目录文件继续保留，不引入磁盘清理迁移。

## 测试决策

- 测试只通过既有服务或 Route Manager seam 验证最终 Home、主供应商引用、错误结果与恢复结果，不 mock 内部 helper 或断言内部调用顺序。
- 保留诊断阶段建立的最小失败测试作为最高层 regression test，先证明 red，再以最小实现转 green。
- 使用已知字面量作为预期配置，避免在测试中复制 production 合并算法。
- 按 vertical slices 逐一完成显式切换、明确同步/供应商保存、启动对账/失败安全，不先批量编写所有测试。
- 最终复跑原始第三方到官方的完整往返场景，并运行相关模块完整测试、格式检查和静态差异检查。

## Test Seams

- **TS-01 — 关闭态显式切换 Route Manager seam**：通过公开的 Profile provider 切换接口，观察主供应商引用和最终直连配置。覆盖 SCN-01、SCN-02、SCN-10、SCN-12。
- **TS-02 — Home Config Service direct-plan seam**：通过公开的 authoritative/automatic direct plan 构造、应用与恢复接口，观察指针 ownership、目标 Home、旧文件保留、并发冲突和补偿。覆盖 SCN-03、SCN-04、SCN-08、SCN-09。
- **TS-03 — 明确 Profile 同步 Route Manager seam**：通过公开的 Managed Profile 显式同步接口，观察当前主供应商对关闭态 Home 的收敛与 best-effort 结果。覆盖 SCN-05、SCN-11，并代表 Import/Restore 共用的后置同步。
- **TS-04 — 共享供应商保存 Route Manager seam**：通过公开的共享供应商更新接口，观察所有主引用 Home 的收敛与整体补偿。覆盖 SCN-06、SCN-10、SCN-11。
- **TS-05 — 启动派生状态对账 Route Manager seam**：通过公开的启动对账接口，观察目录清理、model family 保留、Profile 失败隔离与 External 跳过。覆盖 SCN-07、SCN-11。

## 范围之外

- 对相同 basename 的用户手工目录路径引入更强的来源标记或 ownership 元数据。
- 自动删除孤立目录文件或实现磁盘垃圾回收。
- 改造旧单 Home ProviderService 路径；它只作为诊断对照，不是当前 Codex Profile UI 的真实入口。
- 为模型目录与配置文件增加持久化事务日志。
- 修改 Codex 官方配置格式或上游目录 schema。

## 补充说明

- OpenAI 当前配置参考将 `model_catalog_json` 定义为 Codex 启动时加载的可选模型目录 JSON 路径，因此残留指针具有直接运行时影响。
- 当前最小 regression test 已稳定捕获“route 已切换但指针仍残留”的确切症状；实现必须让该测试转绿，而不是绕过或弱化断言。
