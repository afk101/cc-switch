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
- D2：严格路由字段发生真实冲突时，是将 DB 路由自动收敛为 disabled 并保留用户配置，还是继续维持 enabled 错误态。
- D3：启动恢复是否应同时把严格路由字段收敛到最新端口、token 与协议，并保留全部非路由字段。
- D4：回归测试应覆盖哪些正常变化与真实冲突。
- D5：外部路由字段变化只在应用启动/生命周期操作时检查，还是增加运行期文件监听。

## 已确认设计决定

- 非路由字段不得参与启动恢复的所有权判断。`model`、reasoning、Desktop、plugins、MCP 等字段的正常变化必须被保留，也不得阻止监听器恢复。
- 理由：这些字段会被 Codex/Desktop 或用户正常修改，与本地 Profile 是否仍持有路由无关；整文件 hash 把无关变化耦合进路由可用性，是本次故障的直接设计错误。
- 当 DB 中 Profile 路由仍为 enabled 时，`model_provider` 即使被外部修改，启动恢复也必须自动把当前生效路径接回该 Profile 的本地监听器，而不是把它视为用户退出路由。
- 理由：UI/数据库中的路由开关是生命周期权威状态；用户若要退出路由应明确关闭开关，不能留下“页面显示开启、实际绕过路由”的分裂状态。
- 用户提出有效反例：若用户主动修改实际路由字段却忘记关闭开关，重启时自动覆盖会侵犯用户配置。D2 因此不能直接沿用“enabled 一律强制覆盖”，需区分普通非路由变化与真实路由接管。
- 启动发现当前生效的严格路由已被外部接管时，采用明确的持久化“外部接管”状态：Home 原样保留、不启动 runtime、route 显示关闭、清除旧 Home backup，但保留 `current_provider_id` 供 UI 展示与以后重新启用。
- 外部接管状态必须使后续 disabled 派生状态对账跳过该 Home；用户显式重新启用时清除此状态，并以当时 Home 建立新的 backup 基线。
- 理由：仅设 `enabled=false` 会在下次启动被关闭态 provider 派生逻辑再次覆盖；清空 provider 虽无需 schema，却会无谓丢失用户选择。明确状态能同时保护外部配置与保留 UI 上下文。
- 不增加固定频率轮询或本次引入后台文件 watcher。严格路由所有权在应用启动，以及每次路由生命周期操作前检查；操作完成后校验本次写入结果与状态收敛。
- 若路由开启期间用户改了有效路由配置，下一次操作前先识别为外部接管：Home 原样保留、旧 backup 作废并进入关闭/外部接管状态。
- 用户随后显式重新启用时，必须以启用当刻的当前 Home 作为新的接管前基线并生成新 backup；以后关闭只恢复该新基线中的三个严格路由字段，同时保留关闭瞬间最新的 `model`、Desktop、plugins、MCP 等非路由字段。
- 操作后的检查仅保证该次操作完成时结果正确；没有 watcher 时，操作结束后再次发生的外部修改要到下一次启动或生命周期操作前才会被识别。
- 外部接管自动关闭后不在 UI 保留提示，也不持久展示警告；开关仅静默收敛为关闭。实现仍应写一条不含 token、配置正文或敏感路径的应用日志，供故障排查。
- `model_provider` selector 被外部修改视为用户主动改变请求路径，属于外部接管：不得自动改回，按相同规则静默关闭路由并保护当前 Home。
- `model`、reasoning、Desktop、plugins、MCP 及其他不决定 Profile 本地连接所有权的字段变化不属于外部接管，启动与路由操作必须忽略并保留。
- `config.toml` 不存在、非 UTF-8、TOML 解析失败或结构异常到无法可靠定位当前 provider/严格字段时，按外部接管收敛：文件原样保留、路由静默关闭、不启动 runtime；用户修复后显式重新启用会建立新基线。
- 没有用户显式切换供应商时，任何启动恢复、健康检查、派生状态同步、外部接管识别或自动关闭流程都绝对不得修改 `model`。Home 中用户当前选择的模型是权威值。
- 理由：路由生命周期与模型选择是独立维度；“应用重启”不构成重新应用供应商默认模型的授权。
- 只有用户在 CC Switch 中显式从一个供应商切换到另一个供应商时，才允许按新供应商配置应用其模型；这项授权不得被启动恢复、重新启用同一供应商或自动状态收敛复用。
- 更正被拒绝的 Q10 推荐：不能一概在操作前发现外部接管后终止原操作。用户显式“切换供应商”本身就是覆盖连接配置的授权；应先把操作当刻最新 Home 作为新的接管前基线，再完成供应商切换/路由接管。
- 操作语义需分流：自动启动恢复发现外部接管只静默关闭且不写 Home；用户显式重新启用或切换供应商则以当前 Home 建立新 backup 并继续；用户显式关闭时保留已经外部接管的 Home、清理旧 backup并收敛关闭，不再尝试恢复旧路由字段。
- 每次用户明确授权的新接管（显式启用或显式切换供应商）都必须以操作当刻最新 Home 重建 backup，并取代更早的陈旧 backup。后续关闭恢复到最近一次明确接管前的严格路由字段基线，而非第一次开启时的历史基线。

## 外部方案比较

- 官方 Codex Configuration Reference 将 `model`、`model_reasoning_effort`、`mcp_servers.*` 与 plugins 配置列为独立、受支持的 `config.toml` 设置。这些字段可由用户或 Codex 产品功能正常改变，不是 Profile 路由所有权信号。来源：<https://developers.openai.com/codex/config-reference>（当前重定向到 ChatGPT Learn 官方配置参考）。
- 历史提交 `e1f63f36`（2026-07-14）引入启动 Home 对账，采用整文件 `target_fingerprint` 分类所有权。
- 历史提交 `07e887d9`（2026-07-16）已经发现关闭路径不能整文件恢复，新增 `CodexManagedRouteState`，明确只管理 `base_url`、`wire_api`、`experimental_bearer_token` 三个字段并保留其他配置；但没有同步改造两天前的启动对账路径。
- 方案 A（推荐）：启动恢复复用同一字段级 projection/ownership 语义，只校验三个严格路由字段，并在它们仍由当前 Profile 持有时合并最新目标、保留其他字段。优点是启停语义一致、兼容未来 Codex 新字段；代价是需要把现有私有 helper 抽成启动与关闭共用的单一职责接口。
- 方案 B：对整文件做“移除非路由字段后的规范化 hash”。拒绝倾向强，因为需要穷举所有非路由字段，未来新增字段仍可能造成假冲突，且 TOML 格式/注释变化也会产生无意义差异。
- 方案 C：启动时不检查任何字段直接覆盖。拒绝倾向强，因为真实的 base URL/token 路由冲突会被静默覆盖，破坏现有防止覆盖外部路由的安全边界。
- 审计确认：启用/关闭流程中的整文件 hash 还有另一种用途——在“读取并构造合并结果”到“原子写入”之间做 CAS，防止并发覆盖。应删除的是“整文件 hash 作为启动路由所有权语义”，不是删除所有写前并发校验。
- 当前 route plan 会根据供应商 `upstream_model` 顺带写入 `model`，但 `CodexManagedRouteState` 明确不把它列为严格字段。因此设计上应区分“启用时生成的初始/建议值”和“路由持续拥有的字段”；启动恢复不得把用户后来选择的 model 改回供应商建议值。
- `model_provider` 虽不是严格字段，却决定 `base_url`、`wire_api`、token 的实际生效路径。它不能像普通 `model` 一样完全忽略：若活动 provider 改变，旧 provider 表中仍保留 localhost 并不能证明当前请求仍走本地监听器。
- 最高层 test seam 采用 `CodexRouteManager` 现有真实启动序列：内存 DB + 临时 Home + tracking runtime factory，依次执行 `reconcile_all_profile_derived_state` 和 `restore_enabled_profiles_with_effective_settings`。回归必须断言非路由字段保留、严格字段正确、runtime 恰好启动一次、last_error 为空。
- 当前代码没有监控 `config.toml` 的文件 watcher，也没有路由字段轮询。路由所有权检查发生在应用启动恢复（每次进程启动一次）以及用户触发的启停/切换等生命周期操作中；`lib.rs` 中的 60 秒与 24 小时间隔任务分别用于 usage sync 和数据库维护，与路由检测无关。
- 因此“启动发现冲突后自动关闭 DB 路由”不意味着后台每隔 N 秒扫描。若用户在 CC Switch 运行期间改配置，当前 listener 可能仍运行，但 Codex 会按新配置绕开它；直到下次相关生命周期操作或应用重启才会发现状态分裂。是否新增即时文件监听属于独立产品决策。
- 自动关闭在数据库层技术上可行：`CodexProfileRoute.enabled` 可独立设为 false，DAO 使用单条 UPSERT 保存整条 route，因此单次落库具备 SQLite 语句原子性；启动阶段尚未创建该 runtime，无需停止监听器。
- 但不能直接复用正常 `disable_locked`：正常关闭会先要求当前三个严格字段仍属于 CC Switch，再恢复接管前字段；外部接管场景恰恰无法通过该所有权证明。应有独立的“承认外部接管/放弃路由所有权”收敛动作，禁止写 Home。
- 仅设置 `enabled=false` 不足以长期保护用户配置。当前下一次启动的 `reconcile_all_profile_derived_state` 会对 disabled route 且仍有 `current_provider_id` 的 Profile 重建完整直连配置，可能再次覆盖外部接管结果。
- 可行的最小无 schema 方案是外部接管时原子保存：`enabled=false`、`current_provider_id=None`、`live_backup_json=None`、`recovery_json=None`，保留一个脱敏 `last_error/notice` 供 UI 解释原因。这样后续启动因无 provider 跳过派生状态写入；代价是 UI 丢失原供应商选择，用户下次启用时需重新选择。
- 若产品必须保留供应商选择，则需要新增“Home 外部托管/暂停派生写入”持久化状态；只增加 `enabled=false` 无法区分“CC Switch 管理的直连关闭态”和“用户接管态”，会扩大 schema、迁移、UI 与所有 provider 操作的语义范围。
- 进一步审计结论：候选总体可行，且崩溃一致性简单。不写 Home、不启动 runtime，只需一次 route UPSERT；保存前崩溃则下次重试，保存后即稳定 disabled，无需借用现有 disable recovery WAL。
- 不能复用正常 `disable_locked`，因为它必须先证明三个字段仍由 CC Switch 持有并恢复备份；外部接管场景无法满足。也不能伪造 disable recovery，否则下次恢复会再次尝试写 Home并永久冲突。
- 外部接管后的旧 `live_backup_json` 必须清除：它代表已经放弃的历史 Home 所有权，继续保留会有后续误恢复风险。failovers 与 token store 无需改动；runtime 在启动恢复阶段尚未创建。
- 如新增 `ownership_state=external`（命名待 spec）一类稳定状态，可同时满足：UI 保留 `current_provider_id` 供用户参考、关闭态派生对账跳过 Home、下次显式 enable 基于当时外部配置建立新 backup、再次 disable 能恢复到新的外部基线。
- 若 DB 自动关闭保存失败，则不能宣称已关闭：保持 enabled=true、Home 原样、runtime 未启动，记录安全诊断，下一次启动可重试。
- `model_provider` 的判断不能只比较 selector 字符串：应解析“当前活动 provider 下实际生效的三个严格字段”。selector 虽变但实际仍精确指向该 Profile 时可继续启动；实际指向外部时才判定 external takeover。
- 同一次启动的调用顺序也是 blocking edge：当前 enabled Profile 的派生状态投影在严格所有权检查之前运行，可能先改 `config.toml`。若要求发现外部接管时 Home 原样不动，所有权判断必须前置，或 enabled Profile 的前置投影必须跳过会改 config 的部分。
- listener token 文件若意外丢失，startup 的 `ensure_token` 会生成新 token，而 Home 仍保留旧 token。当前 stale-token 自愈依赖旧整文件 target hash 证明该旧 token 属于 CC Switch；删除整文件语义 hash 后必须提供字段级旧 token proof，否则会把自身凭证重建误判成外部接管。
- 推荐在既有 `live_backup_json` 中增加显式版本化 ownership proof，不新增 DB 列：公共目标由 Profile port/constants 推导，只保存 listener token 的域分离 SHA-256 摘要，不保存明文。新版判定为：当前活动 provider/端口/wire 正确，且 token 等于当前 secret => Current；token 摘要等于 backup 旧摘要 => 可安全升级 stale token；其他 token => external takeover。
- `PROXY_MANAGED` 只保留一次性 legacy fallback：必须同时匹配当前 Profile loopback+port、responses 与占位 token，成功后立即替换成随机 token并升级新版 proof；不能用低熵占位符摘要作为一般所有权证明。
- 旧备份兼容需 fail closed：若 v1 当前 token 等于当前 secret，可直接升级；若 exact legacy triple，可升级；若整文件仍等于旧 target hash，可作为一次性 stale-token migration proof；若 model 等已变化导致整文件不同、token 也不同，则无法安全区分旧 CC Switch token 与外部 token，按外部接管关闭，不能按 token 字符串形状猜测。
- 用户确认旧备份的模糊 token 场景采用上述 fail-closed 策略：静默关闭并保护 Home；用户下一次显式启用后建立新版字段级 ownership proof。
- 用户确认 `config.toml` 缺失时沿用原版 fail-closed 产品语义：不重建文件，静默关闭/标记外部托管；以后显式启用以“文件不存在”为新基线，关闭时移除仅由 CC Switch 创建且无其他内容的配置。
- 临时文件 I/O 错误（权限、磁盘、系统读取失败）不作为外部接管证明：本次不启动 runtime，但保留 enabled、旧 backup 与 ownership 状态，只写脱敏日志；后续启动或路由操作重试。文件缺失、可读取但非 UTF-8/TOML 损坏、结构无法定位则仍按已确认的外部接管静默关闭。

## Spec/Issue 覆盖自审

- 行为主链已覆盖：健康启动、非路由字段变化、selector/严格字段外部接管、缺失/损坏/I/O 错误、旧 token proof 升级、显式启用/切换重建基线、关闭恢复最新基线。
- 安全边界已覆盖：不输出 token、不保存明文 token proof、不触碰 auth.json/其他 Home、写前 CAS 保留、外部接管不写 Home。
- 兼容边界已覆盖：v1 backup、`PROXY_MANAGED`、token 文件丢失、新 ownership proof、原版全局 takeover fail-closed 语义。
- 真实 blocking edges：所有权检查必须早于 enabled 派生配置写入；外部托管状态必须让 disabled 派生对账跳过；显式接管的新 backup 必须在写 Home 前持久化并纳入既有补偿；旧 reconcile recovery 的整文件指纹记录需兼容或迁移。
- 最高层 test seam：真实 app 顺序的 `CodexRouteManager` 启动两阶段 seam；低层补充字段路径、token digest、CAS 与备份版本兼容测试。
- 覆盖矩阵：Issue 01 覆盖字段级 ownership/proof 与 CAS（REQ-01/03/13/14/15/17，SCN-01/02/03/15/17/18/19）；Issue 02 覆盖真实启动、自动外部接管和错误分类（REQ-01—09/16—18，SCN-01—10/21）；Issue 03 覆盖显式启用/切换/关闭的新基线（REQ-02/10—12/17，SCN-11—14/20/21）；Issue 04 覆盖旧版兼容与最终回归（REQ-13—18，SCN-15—21）。
- blocking graph 为 `01 → 02 → 03 → 04`，无环；每个 issue 都能在完成时通过对应 public seam 独立验证，粒度适合 fresh context。

## Execution Context

- Review Base Commit: `5dcd63127b4bb14bd803c269dc0b7759dab43656`
- 上游原版 `upstream/main` 没有本 fork 的 Codex 多 Home/Profile 独立路由；它使用全局 proxy takeover。启动时读取 DB 中 enabled app，调用严格 `set_takeover_for_app(true)`；若恢复失败，立即调用 `set_takeover_for_app(false)` 清除 enabled 状态，而不是无限保留“开启但不可用”。
- 原版 `read_codex_live_settings` 在 `auth.json` 与 `config.toml` 都不存在时明确返回 `codex.live.missing`；严格 takeover 要先读取 live 配置再写接管字段，缺失时失败。第三方 token 写入也明确拒绝空 `config.toml`。因此原版总体原则是“缺失 live 配置不静默凭空重建接管；启动恢复失败则关闭状态”。
- 原版缺失场景与 Profile 版并非完全同构：原版全局 takeover 没有每 Profile Home backup、listener token proof、外部接管状态。但其 fail-closed 产品语义支持本设计将配置缺失视为无法恢复并关闭，而非自动创建用户文件。

## 被拒绝方案

- 继续使用整份 `config.toml` hash，只对白名单字段做 hash 归一化：拒绝。它仍会把未来新增的 Codex/Desktop 字段错误纳入路由所有权，维护上属于默认拒绝未知字段。
- 启动时无条件覆盖整份 `config.toml`：拒绝。会丢失 Codex/Desktop 与用户合法配置。

## Spec/Issue 覆盖自审

- 待 design tree 收敛后填写。

## 未验证边界

- 是否只影响登录启动，还是普通退出重启也影响。
- 是否只影响 Codex Profile 路由，还是全局 proxy takeover 也影响。
- 是否与端口占用、异步启动时序、凭据恢复或运行时注册有关。
