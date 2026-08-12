# Findings & Decisions

## Requirements

- 诊断“应用开机启动后，已启用的 Codex Profile 路由必须手动关闭再开启才能连接”的根因。
- 先建立会因端口不可连接而失败、手动开关等价恢复后转绿的自动反馈环。
- 当前阶段只诊断；根因经用户确认后，再进入修复方案与实施流程。

## Findings

- 当前分支为 `feature/v3.19.1`，与 `origin/feature/v3.19.1` 同步。
- 工作区原有未跟踪目录 `.scratch/wiscode-codex-x-api-key-401/`，与本任务无关，保持不动。
- 历史中存在近期提交 `b9562159 fix(codex): 记录启动路由恢复阶段诊断`，需要确认它位于哪个分支、修改了什么，以及是否只增加诊断而未修复服务可用性。
- 当前测试搜索没有发现直接断言“启动恢复后监听端口可连接”的用例；已有测试更多覆盖 route lifecycle 状态或前端 toggle。
- 当前 HEAD 实际为 `e8859207`，`b9562159` 是其直接祖先且已经同时存在于本地与远端 `feature/v3.19.1`；因此用户运行的代码包含该提交。
- `b9562159` 的提交目标是“记录启动路由恢复阶段诊断”，主要增加失败阶段分类、脱敏日志和测试观察能力；提交标题虽然是 `fix`，但从变更说明看并未声称修复启动后服务不可达本身。
- 该提交修改 `route_manager.rs`、`route_runtime.rs`、proxy server/error/handler，并附带一份 startup restore diagnostics spec；需要用真实可连接性反馈环检验其恢复成功路径。
- `b9562159` 附带的 spec 明确写明“本次只增强可观测性，不改变启动恢复控制流，也不增加重试或修复具体根因”；因此它不是用户所理解的服务恢复修复，而是为了这次复现留下阶段证据。
- 2026-08-12 10:02:31 的真实日志捕获到 `codex-api` 启动恢复失败：`stage=reconcile_home category=derived_state`。
- 同一日志直到 10:14:35 才出现 `127.0.0.1:15722` 的 server/runtime 监听成功；该时刻对应用户手动开关后的恢复。因此有约 12 分钟的“数据库仍为 enabled/Home 仍指向本地端口，但没有监听器”的不可服务窗口。
- 当前手动恢复后的进程正在 `127.0.0.1:15722` 监听，说明监听端口本身、运行时启动与供应商转发并非持续性故障。
- 过去几次真实启动失败阶段也有 `build_plan/database`，但 2026-08-11 与 2026-08-12 最近两次均稳定落在 `reconcile_home/derived_state`，本轮优先诊断该当前重复根因。
- 现场 `~/.codex-api/config.toml` 的修改时间为 2026-08-12 10:14:35，与手动开关后 listener 成功时间完全一致；当前文件指纹也与重新启用后 DB route backup 的 target 指纹相等，说明手动开关确实重建了 Home 所有权基线。
- 手动开关后新 backup 的 `previous_content`（即关闭阶段字段级恢复后的 Home）保留了 `js_repl`、`desktop.followUpQueueMode`、plugins、`mcp_servers.node_repl` 等 Codex/Desktop 自有字段。这些字段在当前实现中不属于三个严格路由字段。
- 更关键的是，新 backup 的 `previous_content` 中 `model = "WisGPT-5.6-Luna"`，重新启用后的目标为 `model = "WisGPT-5.6-Terra"`。关闭流程只恢复 base_url、wire_api、listener token，不改 `model`，所以 Luna 是关闭前已启用 Home 中的实际值；它与 route 根据供应商重建的 Terra 目标不同。

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| feedback loop 必须检查实际监听/请求可用性 | 用户症状是“连不上”，仅检查数据库 enabled 状态或恢复函数成功会产生假绿 |
| 诊断阶段不修改生产逻辑 | 遵循 diagnosing-bugs 根因确认门禁 |
| 以当前 HEAD `e8859207` 为权威状态 | 最近提交已在当前分支，不以先前会话中的旧 HEAD 推断 |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| 首次追加 findings 时使用了不存在的旧标题，补丁未应用 | 先读取当前文档，再按实际的 `## 调用链与 Data Flow` 标题定位；未重复失败操作 |
| 首次 targeted test 使用 `--exact` 但未给完整模块路径，运行 0 tests | 改用唯一测试名过滤；随后实际运行 1 个测试并通过，未重复原命令 |

## Resources

- `CONTEXT.md`
- `.scratch/codex-startup-route-unreachable/findings.md`
- commit `b9562159`

## Visual/Browser Findings

- 暂无。

## 反馈环

- 现场 replay 输入：`~/.cc-switch/logs/cc-switch.log` 中 2026-08-12 的 startup failure 与后续 listener 日志。
- 可执行命令：`.scratch/codex-startup-route-unreachable/check-startup-route-log.sh /Users/qihoo/.cc-switch/logs/cc-switch.log codex-api`。
- 首次运行结果稳定 RED（exit 1）：失败记录为 10:02:31 `reconcile_home/derived_state`，之后首次监听为 10:14:35 的 15722。
- 该 loop 是真实 trace replay、秒级、无人值守；替换为修复后的新启动日志时，没有 startup failure 即 GREEN。

## 最小复现

- 已最小化到单个已启用 Profile `codex-api`：应用启动 → `reconcile_home` 失败 → 不进入 runtime start → 15722 无监听 → 手动 toggle 后同一端口成功监听。
- 每个剩余元素都不可移除：去掉 enabled route 不会启动恢复；去掉 reconcile failure 会继续启动 runtime；去掉本地 Home 指针则客户端不会表现为连接本地端口失败。

## Hypotheses 与实验

- H1（并发 Home 写入）：缺少启动窗口内第二个写者的日志；失败在启动同步阶段稳定重现两天，而手动开关后的配置 mtime 只对应开关时刻。降级为低概率。
- H2（开启态重复/错误对账）：前置模型目录投影确实可能写 `config.toml`，但现有 `startup_restore_uses_database_versions_for_enabled_primary_and_failover` 已覆盖这一串行流程并成功恢复 listener；它不是单独充分原因。
- H3（模型目录/文件瞬态）：若启动时目录或 managed catalog 不存在/不可写，则复现实验应在文件系统操作处失败；当前手动 toggle 成功排除持续权限问题，但不排除瞬态。
- H4（供应商配置无效）：若配置本身无效，使用当前数据库 provider settings 的独立投影也会稳定失败；因手动 toggle 成功，当前优先级低。
- H5（数据库锁）：若 DB 锁是原因，错误应发生于含 DB 访问的第一轮全量对账或 plan 构造；当前明确阶段为第二轮 `reconcile_home`，优先级降低。
- H6（已确认机制，待最终回归测试）：路由启用期间 Codex/Desktop 或用户合法更新非严格路由字段（现场至少 `model` 已从路由目标 Terra 变成 Luna）；启动恢复使用整文件指纹判定 Home 所有权，因任意字段变化返回 `CodexLiveConfigConflict`，随后跳过 listener。手动关闭使用字段级恢复、保留非路由字段，再次启用重建 backup，故恢复。
- H6 验证：`cargo test restoring_enabled_profile_home_preserves_external_edit_and_isolates_failure --lib -- --nocapture` 实际运行 1 个测试并通过。该测试确认 Home 整文件发生变化时，恢复会记录与现场相同的 `stage=reconcile_home category=derived_state`，且 runtime 不启动；健康 Profile 才会启动。

## 调用链与 Data Flow

- Tauri startup task 串行执行：`reconcile_all_profile_derived_state(db)` → `restore_enabled_profiles_with_effective_settings(db)`。
- restore 对每个 enabled Profile 执行：锁 Profile → recover pending → 读 route → ensure token → build plan → `reconcile_enabled_home` → `start_restored_runtime`。
- 当前现场在 `reconcile_enabled_home` 返回错误，故 `start_restored_runtime` 完全未调用；这与日志中 10:02 后没有 15722 listener 一致。
- 启动前置的 `reconcile_all_profile_derived_state` 对 enabled route 只投影模型目录，不会改写完整 `config.toml`；因此 H2 中“第一轮完整配置写坏第二轮”的推断不成立。
- `reconcile_enabled_home` 的所有权门禁只有三条成功路径：当前 Home 已等于新目标、等于 route backup 中上次写入的目标、或符合旧版代理格式。其余情况统一返回 `CodexLiveConfigConflict`，并阻止监听器启动。
- 关闭路由的 `restore_profile_decoded_backup` 已经采用字段级所有权校验，只管 base_url、wire_api、listener token，并将其余当前字段原样保留；启动恢复没有复用同一粒度，而是仍比较整文件指纹。这解释了同一份 Home 为何“自动启动失败、手动关开成功”。

## 根因

- 已确认：已启用 Profile 的启动恢复仍用整份 `config.toml` 指纹证明路由所有权；Codex/Desktop 或用户只要修改任一非严格路由字段（现场已证明 `model` 与大量 Desktop/plugin 字段存在并被字段级关闭流程保留），当前指纹就与 route backup target 不同。`classify_profile_reconcile` 随即返回 `CodexLiveConfigConflict`，restore 在 `start_restored_runtime` 前中止，于是 DB 显示 enabled、Home 指向 15722，但端口无人监听。手动关开之所以成功，是关闭路径已按三个路由字段恢复并保留其他字段，随后开启基于当前 Home 重建 backup 和 target。

## 开放决策

- D1：启动恢复应以哪些严格路由字段证明 Home 仍归当前 Profile 管理。
- D2：严格路由字段发生真实冲突时，监听器应 fail closed，还是仍启动并暴露不可达/错路由状态。
- D3：启动恢复是否应同时把严格路由字段收敛到最新端口、token 与协议，并保留全部非路由字段。
- D4：回归测试应覆盖哪些正常变化与真实冲突。

## 已确认设计决定

- 非路由字段不得参与启动恢复的所有权判断。`model`、reasoning、Desktop、plugins、MCP 等字段的正常变化必须被保留，也不得阻止监听器恢复。
- 理由：这些字段会被 Codex/Desktop 或用户正常修改，与本地 Profile 是否仍持有路由无关；整文件 hash 把无关变化耦合进路由可用性，是本次故障的直接设计错误。

## 外部方案比较

- 待补充：字段级所有权、合并写入与冲突处理方案将以仓库既有关闭路径、历史提交及 TOML 编辑行为为一手依据比较。

## 被拒绝方案

- 继续使用整份 `config.toml` hash，只对白名单字段做 hash 归一化：拒绝。它仍会把未来新增的 Codex/Desktop 字段错误纳入路由所有权，维护上属于默认拒绝未知字段。
- 启动时无条件覆盖整份 `config.toml`：拒绝。会丢失 Codex/Desktop 与用户合法配置。

## Spec/Issue 覆盖自审

- 待 design tree 收敛后填写。

## 未验证边界

- 是否只影响登录启动，还是普通退出重启也影响。
- 是否只影响 Codex Profile 路由，还是全局 proxy takeover 也影响。
- 是否与端口占用、异步启动时序、凭据恢复或运行时注册有关。
