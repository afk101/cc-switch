# Findings & Decisions

## Requirements

- 诊断已选 Codex Profile 使用某供应商时，编辑该供应商的 `model` 后点击保存，Profile Home 下的 `config.toml` 未立即更新的问题。
- 同时核对此前 `base_url` 保存不立即生效的修复为何没有覆盖本次现象，区分修复回退、遗漏字段和不同调用链。
- 在修复前建立可重复、快速、无人值守且能捕获用户确切症状的反馈环。
- 找到调用链上最早的错误来源后，先向用户报告并等待根因确认；本阶段不直接修复。

## Findings

- 当前分支为 `feature/v3.19.2`，跟踪 `origin/feature/v3.19.2`；工作区只有两个与本任务无关的既有未跟踪 `.scratch` 目录。
- 项目已有 Codex 配置、供应商保存和 Profile 派生状态相关 Rust 测试基础，可优先在后端 service seam 构建真实保存链路的回归复现。
- 用户可见症状包含两个字段：此前出现过的 `base_url`，以及当前新发现的顶层 `model`；共同触发方式是编辑当前 Profile 正在引用的供应商并保存，切换供应商后才会写入 Profile Home。
- 先前修复有完整的 spec 和 7 个 issue，说明问题曾按“共享供应商保存后收敛所有 Profile 派生状态”设计处理；需要验证当前 v3.19.2 是否仍执行同一入口，而不能仅凭历史提交判断已覆盖。
- `src-tauri/src/services/provider/mod.rs` 已包含 Codex provider update 的测试工具和原子 preflight/commit 测试区域，是建立真实保存反馈环的首选位置。
- 旧 spec 明确要求“路由关闭的 Profile 保存主供应商后重建完整直连配置，Base URL、模型和供应商相关字段保持一致”，因此当前 `model` 症状属于旧修复承诺范围，不是新需求。
- 当前 `ProviderService` 局部测试只证明 prepare 不写库、commit 不改全局 `~/.codex/config.toml`；真正的 Profile Home 收敛应由更高层 route manager 测试覆盖，需定位其现有回归用例。
- 命令层已有高层回归 `shared_provider_save_updates_disabled_primary_profile_direct_config`，覆盖关闭态主引用 Profile 的 Base URL、通用配置、无关 Profile 和私有文件，但供应商 fixture 只把 `old-model/new-model` 放进 `modelCatalog`，没有把顶层 `model` 写入供应商 `config`，因此现有测试无法捕获用户当前报告的默认模型不更新。
- 当前命令入口仍把 Codex 保存委托给 `codex_route_manager.update_shared_provider`，说明先前修复入口没有整体丢失；需要用新增顶层 `model` 断言判断遗漏发生在投影内容还是保存编排。
- 历史实现提交为 `5dda2041 fix(codex): converge profile state on provider save`，当前分支包含该提交。
- 现有高层 Base URL 回归测试在保存前通过 `build_direct_provider_plan` 写入旧供应商，保存时调用真实 `update_shared_provider`，正好覆盖用户操作链路；它只缺少顶层 `model` 输入与断言。
- `update_shared_provider` 当前仍先生成有效供应商配置，再进入 `with_provider_home_update`，最后提交数据库；因此新增模型用例若变红，能够把问题限定在有效配置生成或 Home 直连投影，而不是前端没有调用保存入口。
- 工具链阻断已定位：PATH 首个 `cc` 是 `/Users/qihoo/.nvm/versions/node/v22.22.0/bin/cc`，系统编译器是 `/usr/bin/cc`，Xcode Clang 是 `/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/clang`；`ar` 为系统/Xcode 工具。
- 显式使用系统 Clang 后，新增诊断用例进入真实保存链路并稳定失败：保存调用完成后，目标 Profile Home 仍为 `model = "old-model"`，没有写入 `model = "new-model"`。
- 失败文件同时保留 `model_provider = "custom"`、`base_url = "https://example.com/v1"` 和 provider 表，说明并非整个文件未生成，而是顶层模型在投影/合并时被旧值保留。
- `CONTEXT.md` 与 ADR 均把共享供应商定义为权威来源、Profile Home 定义为可重建派生状态；ADR 还明确排除了“仅保存数据库，等切换后刷新”，因此当前行为直接违反已接受的一致性决策。
- 搜索发现 `home_config.rs` 已有多组“保留 user-selected/user-model”的测试，而切换 provider 的测试明确断言改成新 provider 默认模型；这使“同 provider 保存时保留当前模型”的策略分支成为最高优先级嫌疑。
- `update_shared_provider` 确实将 `prepared_provider` 克隆为 `effective_provider`，只合并公共配置后传入 Home 投影；没有在该入口显式删除 `model`，假设 2 的概率下降。
- `prepare_provider_save_projection` 对关闭态主引用明确调用 `build_automatic_direct_provider_plan`；该函数固定选择 `PreserveUserModel`。普通切换则调用 `build_direct_provider_plan`，固定选择 `ApplyProviderModel`。保存和切换的策略分叉已被代码直接确认。
- 路由管理器已有测试把产品语义写成“关闭态再次选择当前供应商不得改写 Home，切换到其他供应商才应用其模型”；这解释了为何切走再切回会生效，但把“用户在 provider 编辑表单显式更改默认模型”与“Codex 在 Home 内自行选择模型”混为同一种保留场景。
- `merge_automatic_direct_provider_projection` 的实现对顶层 `model` 和所有 `model_reasoning_*` 字段直接 `continue`，即无条件跳过 provider 投影值；因此只要 Home 已有旧模型，计划生成阶段就必然保留旧模型。
- Git 历史显示：原修复 `5dda2041`（8 月 3 日）在关闭态保存时调用 `build_direct_provider_plan`，会应用 provider 模型；后续提交 `29768fe5`（8 月 12 日）把这一行改成 `build_automatic_direct_provider_plan`，并引入 `PreserveUserModel`。这是原修复之后发生的行为回退，不是原修复从未进入当前分支。
- `29768fe5` 的提交标题是 `fix(codex): 自动投影保留用户模型`；对应已解决 issue 27 明确规定：共享供应商保存时顶层 `model` 与 `model_reasoning_*` **永远保留**，只有显式切到不同供应商才应用目标默认模型。
- 紧随其后的 `7b69f796 fix(codex): 权威同步供应商受管字段` 专门让 Base URL、provider name、headers 等受管字段权威同步，同时继续断言 `model = "user-selected-model"` 保留。因此 Base URL 的旧问题已被后续补齐，而 `model` 不更新是当前显式策略造成的新冲突。
- 现有策略只有最终 Home 内容，没有记录模型来源：它无法区分“用户直接在 Codex/Profile Home 内选的模型”（应保留）和“上次由 provider 默认值投影出的旧模型”（provider 编辑后应更新）。二者都被同一个无条件 `continue` 处理。
- 单变量断言证明保存后的 provider 数据库配置已包含 `model = "new-model"` 且不含旧模型；紧接着读取 Profile Home 仍得到 `model = "old-model"`。假设 2 已排除，错误首次出现在 Home 自动投影。
- 直接运行既有 Base URL 高层回归 `shared_provider_save_updates_disabled_primary_profile_direct_config` 为绿灯（0.03 秒）；说明当前 Base URL 权威同步修复仍有效，当前 `model` 问题不是整条旧修复失效。

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| 使用独立任务目录 `codex-profile-current-provider-save-sync` | 与此前同类问题区分，同时保留关联线索 |
| 优先在 Rust provider service / Profile 派生状态 seam 建立反馈环 | 能直接执行真实保存与 Home 配置投影链路，比前端 mock 更能捕获用户症状 |
| 根因确认前不修改生产实现 | 遵循 diagnosing-bugs 的根因确认门禁 |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| 初始宽泛搜索包含大量无关模型获取代码 | 下一步缩窄到既有 Profile 保存同步测试、provider service 和历史修复提交 |
| 首次运行精确 Rust 测试未进入测试逻辑；`cc` 实际输出“检测运行中的代理”，导致 `objc2-exception-helper` 不识别 `-MD` | 不重复原环境；先解析系统 Apple Clang/`ar` 路径，再显式设置目标专用编译器运行反馈环 |
| PATH 中 NVM 的 `cc` 遮蔽 `/usr/bin/cc` | 下一次使用 `CC=/usr/bin/clang CXX=/usr/bin/clang++ AR=/usr/bin/ar` 显式覆盖，不修改用户全局环境 |
| 第一次 findings 补丁因上下文原句不匹配而未应用 | 先读取现有 findings，再按实际章节精确追加；不重复原补丁 |
| 重新编译诊断测试时包装层只回传到“Running unittests”，未给最终摘要 | 检查确认进程已退出后，不重复 cargo 命令；直接运行已生成的精确测试二进制取得完整断言结果 |

## Resources

- `/Users/qihoo/Documents/A_Code/Fork/cc-switch/CONTEXT.md`
- `/Users/qihoo/Documents/A_Code/Fork/cc-switch/docs/adr/0001-codex-profile-derived-state-convergence.md`
- `/Users/qihoo/Documents/A_Code/Fork/cc-switch/.scratch/codex-profile-provider-save-sync/`

## Visual/Browser Findings

- 暂无；当前阶段采用自动化后端复现。

## 反馈环

- 正确 seam 已定位到命令层高层测试 `shared_provider_save_updates_disabled_primary_profile_direct_config`；下一步新增一个只改变顶层 `model` 的独立回归用例，命令将使用精确测试名运行。
- 已新增 `shared_provider_save_updates_disabled_primary_profile_model`，但第一次命令在依赖编译阶段被 PATH 中错误的 `cc` 阻断，尚未获得 Bug 红/绿判定。
- 使用 `CC=/usr/bin/clang CXX=/usr/bin/clang++ AR=/usr/bin/ar cargo test --manifest-path src-tauri/Cargo.toml shared_provider_save_updates_disabled_primary_profile_model -- --nocapture --test-threads=1` 后获得红灯；测试 0.04 秒内完成，且不会读写真实 `~/.codex-api`。
- 精确失败输出中的目标配置仍含 `model = "old-model"`，已建立可重复、快速、无人值守的反馈环。
- 加入数据库节点断言后直接运行测试二进制，数据库新模型断言通过，最终只在 Home 新模型断言处失败（`provider.rs:409`）；证明测试具备正确的阶段定位能力。
- Base URL 对照用例在相同二进制下通过：`1 passed; 0 failed`。

## 最小复现

- 目标最小场景：一个关闭路由的 Codex Profile，主供应商配置初始 `model = "old-model"`；通过共享供应商保存入口改成 `model = "new-model"`；不做供应商切换，直接读取该 Profile Home 的 `config.toml` 并断言新值。
- 只保留一个内存数据库、一个临时 Profile Home、一个主供应商引用和一次保存；不需要模型目录、通用配置、私有文件、故障转移或运行中路由。

## Hypotheses 与实验

1. **高**：直连配置投影把 Home 当前顶层 `model` 视为用户覆盖项并保留，覆盖新 provider 的默认模型。证伪：若直连 plan 生成前后已是 `new-model`，则排除。
2. **中**：`update_shared_provider` 构造的有效 provider 没有携带新顶层 `model`。证伪：若有效 provider 的 `settings_config.config` 含 `new-model`，则排除。
3. **中**：保存与切换使用不同投影模式，保存只更新 provider 表，切换完整替换模型。证伪：若两条路径进入同一模式和同一 merge 函数，则排除。
4. **低**：后续公共配置或恢复步骤把已写入的新模型覆盖回旧值。证伪：若生成的 plan 在写盘前已经是旧模型，则排除。

当前状态：假设 1 和 3 已确认；假设 2 被数据库节点断言排除；假设 4 被“计划 merge 本身无条件跳过模型”及写盘后只有旧值排除。

## 调用链与 Data Flow

- 保存：`commands::provider::update_provider_for_app` → `CodexRouteManager::update_shared_provider` → `with_provider_home_update` → `prepare_provider_save_projection` → 关闭态主引用走 `build_automatic_direct_provider_plan(PreserveUserModel)` → 应用 Home plan → 提交 provider 数据库。
- 切换到不同 provider：`switch_provider_internal` → `switch_direct_provider_locked` → `build_direct_provider_plan(ApplyProviderModel)` → 应用 Home plan → 保存 Profile 引用。

## 根因

- 技术根因已形成证据闭环，等待用户确认：`29768fe5` 为保护 Home 中用户自选模型，将共享 provider 保存改走 `PreserveUserModel`；其 merge 无条件跳过顶层 `model`。系统没有模型来源/provenance，无法区分用户自选值和上次 provider 投影的默认值，导致 provider 编辑页明确保存的新默认模型也被当成用户模型保留。
- 这也解释了切换行为：切换到不同 provider 走 `ApplyProviderModel`，因此切走再切回时新默认模型才写入。

## 未验证边界

- 路由开启与关闭状态是否走不同同步分支。
- 路由开启态的 `model` 生效语义（Home 只更新目录、运行时换 snapshot）尚未用用户场景复现；当前已确证的是关闭态 Managed Profile。
- 当前供应商、主供应商引用、故障转移引用是否处理不同。

## 外部知识缺口

- 已核对 OpenAI 官方配置参考：用户级配置位于 `$CODEX_HOME/config.toml`；顶层 `model` 是 Codex 实际使用的模型，`model_provider` 指向 `model_providers` 中的 provider。官方没有规定 CC Switch 在“provider 默认模型”和“用户直接修改 Home 模型”之间的覆盖策略，因此该所有权必须作为本项目的显式产品决策。
- 官方一手来源：https://developers.openai.com/codex/config-reference（当前重定向到 ChatGPT Learn 配置参考）。
- 待本地事实核验：保存链路能否同时得到旧、新 provider 模型以实现变更感知；最高层现有测试 seam 是否足够覆盖两种模型所有权场景。
- 本地只读核验完成：`with_provider_home_update` 在提交前同时持有待保存的新 provider 和从数据库读取的旧 provider；旧值当前仅用于失败补偿。因此可在既有事务内比较原始 provider 顶层 `model`，无需增加持久化 provenance 字段。
- 比较应使用数据库旧 provider 与 `prepared_provider` 的原始供应商配置，不能用已合并公共配置的 `effective_provider`，否则公共配置变化可能被误判为供应商模型变化。
- 最高层现有 seam 是 `commands/provider.rs::update_provider_for_app` → `update_shared_provider` 的真实 DB + 临时 Home 测试。现有诊断用例可覆盖“模型改变时更新”；还需补一个“模型不变、只改 Base URL 时保留 Home 用户模型”的精确对照。

## 外部方案比较

| 方案 | 行为 | 优点 | 已知失败模式 |
|------|------|------|--------------|
| 每次 provider 保存都覆盖 Home `model` | provider 始终权威 | 规则简单，保存立即一致 | 仅修改 Base URL、名称等无关字段也会抹掉用户在 Codex 内的选模 |
| provider 保存永远保留 Home `model` | Home 始终权威 | 不干扰用户临时选模 | 正是当前 Bug；provider 编辑页保存的新默认模型不会生效 |
| 仅当 provider 的默认 `model` 本次发生变化时覆盖（推荐候选） | 模型变化时 provider 权威；其他字段保存时 Home 模型权威 | 同时表达两种用户意图，不需要把无关保存解释成改模型 | 必须定义删除模型、多个 Profile、reasoning 字段等后续边界 |

## 开放决策

- Q1（已决定）：采用“每次 provider 保存都覆盖 Home `model`”的强权威语义，不以 provider 模型是否变化为条件。用户明确选择方案 A。
- 后续依赖 Q1：模型删除语义、reasoning 字段是否联动、多个引用 Profile 的扇出范围、enabled Profile 行为与冲突处理。
- Enabled/Managed 事实核验：Home 顶层 `model` 决定 Codex 客户端发出的请求模型，provider 路由本身由 runtime snapshot 决定。当前 provider 保存仅更新模型目录与 runtime snapshot，不更新 Enabled Home 的顶层 `model`。
- 仅更新 runtime 不足以实现方案 A：Native Responses 会沿用 Home/请求模型；转换路径在旧模型仍位于 catalog 时也优先尊重请求模型。因此若 Enabled Profile 也在覆盖范围，必须在保持本地 listener 严格路由字段不变的同时写 Home 顶层 `model`。
- 现有高层测试已证明 runtime 热替换不会中断在途请求；新增 Home 模型投影只影响保存后的新请求/新会话，不应迁移已经进入转发过程的请求。
- Q2（已决定）：采用 A2，覆盖所有以该 provider 为主供应商的 Managed Profile，包括 enabled 与 disabled；enabled 写模型时必须保留 listener 严格路由字段与运行时生命周期，在途请求保持进入时快照。External Home 继续不参与自动投影。
- 待核验后决定：provider 顶层 `model` 缺失/被清空时，是删除 Home 模型、保留旧值，还是拒绝保存。
- 空模型事实核验：custom provider 的 catalog 非空时，UI 会回退到 catalog 第一项；catalog 为空时可保存无 model。official provider 默认不显示模型输入且空 config 很常见。后端不要求 model，合法 TOML 中缺失或空字符串都可通过。
- full direct 投影在 provider 缺失 model 时会删除 Home 旧 model；当前自动保存则保留。enabled 的 custom 切换仅在目标有非空模型时覆盖，显示出当前不同路径的删除语义并不一致。
- Q3（已决定）：采用 A3。provider 未声明有效非空 model 时，删除所有受管目标 Home 的旧顶层 `model`；显式空字符串归一化为未设置，不写 `model = ""`。custom 与 official 使用一致删除语义。
- 后续 frontier：`model_reasoning_*` 是否随 provider 强权威覆盖/删除，还是继续归 Home 用户所有。
- 推理字段事实核验：Provider 原始 TOML 编辑器可编辑任意 `model_reasoning_*`；高级区 Chat reasoning 元数据是另一套代理转换配置。后端不要求 reasoning 与 model 配套，二者可独立存在。
- 当前行为不一致：disabled full-direct 切换会应用/删除 provider reasoning；disabled 自动保存与启动对账始终保留 Home reasoning；enabled 保存和切换也保留 Home reasoning。
- 当前保护按任意 `model_reasoning_` 前缀匹配，不是已知字段白名单。provider 删除 reasoning、只改 reasoning 或切换模型时，来源同样无法判断。
- Q4（待决定）：Q1 的 provider 强权威是否扩展到全部顶层 `model_reasoning_*`（声明则覆盖、缺失则删除），还是这些字段继续永久归 Home 用户所有，或采用“声明覆盖、缺失保留”的混合语义。
- 用户对 Q4 中“CC Switch 可配置 reasoning”的位置提出疑问，尚未作出 Q4 决策。
- UI 精确核验：Codex 供应商编辑页下方始终渲染标为“`config.toml (TOML)`”的原始文本编辑器；没有 `model_reasoning_effort` 的独立可视化输入控件，用户必须在该文本框手写。代理高级区的“思考能力/思考等级”写入 `meta.codexChatReasoning`，与 Home 顶层 reasoning 不同。
- 用户要求“用原始 CC Switch 原版逻辑”，但该词可能指本 Fork 在 `29768fe5` 前的完整直连投影，也可能指 Git 上游原版；Q4 暂不落定，先核验 upstream/remotes 和对应版本行为。
- Git 上游核验：`upstream=farion1231/cc-switch.git`；当前 Fork 的发布基线是上游 v3.19.2（peeled commit `43eaf073`），不是 v3.19.0/v3.19.1。本地还存在 v3.19.1=`28529620`、v3.19.0=`c0ff89b9`。
- 上游原版 v3.19.2 保存当前 Codex provider 时，把 provider 的整段 `settings_config.config` 作为权威 Live `config.toml` 写入；没有 old/new model 比较，也没有 `model`/`model_reasoning_*` 保留分支。字段修改或删除都会同步修改或删除 Live。
- 原版切换 provider 同样整段替换目标 config；custom 只额外注入 token，official/custom 对 model/reasoning 的整体覆盖语义一致。代理接管也以 provider 整段 config 为基线，仅叠加 catalog/token/route 字段。
- 上游原版没有 `src-tauri/src/codex_profile/`，不存在多 CODEX_HOME/Profile 的可直接复用规则。将原版整文件权威语义映射到多个 Managed Profile 是 Fork 的产品决策。
- 用户要求采用原版逻辑，因此 Q4 记为 A4：provider 中的 `model` 与全部 `model_reasoning_*` 均权威覆盖；缺失则删除。
- Q5（已决定）：采用 B5。将上游原版的 provider 权威语义限定到 provider 受管字段；`model`、全部 `model_reasoning_*`、Base URL、provider 表等按 provider 覆盖/删除，同时保留每个 Profile 独有的 Desktop、插件和未知扩展。enabled 仍叠加并保留本地 listener 严格路由字段。
- 后续 frontier：provider 权威模型同步是否也在应用启动对账时执行，还是只由明确的保存/切换动作授权。
- 启动事实核验：上游原版 v3.19.2 普通启动不会把当前 provider 整段 config 写回 Live；只有配置导入、显式同步、保存/切换等明确动作写 Live。代理接管恢复通常幂等返回，必要重建时也只改 route 字段及可解析的 upstream model，不全量同步 reasoning。
- 当前 Fork 启动对账对 disabled/Managed 保留 Home `model` 与全部 `model_reasoning_*`，权威同步其他 provider 字段/MCP；对 enabled/Managed 只更新 catalog/runtime 和 listener 严格字段，Home model family 保留。External 跳过。
- 更正：先前认为 enabled Home 修改 model family 后必须同步重建 disable 恢复 baseline，这一判断不准确。当前 disable 在最新 Home 文档上只恢复 `base_url`、`wire_api`、`experimental_bearer_token` 三个严格路由字段，不整份恢复 `previous_content`；因此保存时写入的新 model/reasoning 会自然保留到 disable 之后，Profile 扩展也保持不变。
- Q6（已决定）：采用 A6。只有明确保存 provider、切换 provider 或显式同步动作应用 provider 的 model family；普通应用启动对账继续保留各 Home 当前 model/reasoning，不引入启动重置语义。
- enabled→disable 边界已由实现事实闭合：无需新增产品决策或重建备份 baseline；应增加高层测试证明保存后的新 model family 在 disable 后继续存在，且只恢复严格路由字段。
- Design-tree 最终审计确认可沿用既有合同：同 provider 的全部 Managed 主引用（enabled/disabled）全量扇出；failover-only 不改 Home、enabled 仅换 runtime；External 永不自动写；provider key rename 继续拒绝；CAS 冲突、批次反向补偿与 DB commit 补偿语义保持不变。
- 仍有三个独立开放决策：① common config 与 provider raw config 的 model-family 冲突优先级；②显式 Sync、导入/云恢复的 Profile 扇出授权；③交互式保存进程崩溃残留与 A6 启动保留语义的取舍。
- Common config 事实：当前 effective settings 先取 provider 存储值，再合并 common snippet，标量冲突由 common 覆盖；自动提取 common 会删除 `model`，但不会排除全部 `model_reasoning_*`，手写 snippet 可包含任意合法 TOML。
- Q7（已决定）：采用 A7，沿用现有/上游 effective-config 合同。Provider 存储配置先合并 Common Config，model-family 冲突时 Common Config 最终优先；Profile 投影以合并后的有效配置为准。
- Q8（已决定）：采用 A8。保存、切换、手动 Sync、数据库导入与云恢复均属于明确同步动作；遍历所有 Managed Profile，按各自主 provider 同步 model-family，External 跳过。
- Q9（已决定）：采用 B9。手动 Sync、Import/Restore 对 Managed Profile 逐个 best-effort；单个 Home 失败不回滚已成功 Home，也不阻止后续 Profile，允许短期部分新/部分旧。Import/Restore 的 DB 保持已导入状态。
- B9 不改变交互式 provider save 的既有原子合同：保存单个 provider 仍要求全部引用 Profile 成功，否则恢复已写 Home 并拒绝 DB commit。
- B9 与 A6 共同决定崩溃语义：不新增 provider-save/import 的跨文件 journal；普通启动不重置 model-family；未完成或失败的 Profile 等待下一次明确 Save/Switch/Sync/Import/Restore 收敛。ADR 需明确 model-family 的显式收敛例外，避免继续声称启动必然全量重建。
- Q10（已决定）：采用 A10。best-effort 部分失败返回“操作完成但有 Profile 同步警告”；展示失败 Profile 名称和脱敏原因，不包含 token/API Key/配置正文。Import/Restore 的 DB 成功状态保持成功。

## 被拒绝方案

- “provider 保存永远保留 Home 模型”：拒绝，因为会保留当前 Bug。
- “仅当 provider 默认模型本次发生变化时覆盖”：用户未采用推荐方案；因此即使只保存 Base URL 等其他字段，也应重新应用 provider 的默认模型。

## Spec/Issue 覆盖自审

- Design tree frontier 已清空，用户已用“ok，写好，然后 `$implement` 执行直到完成”确认完整共同理解与实施授权。
- 拟定 vertical issues：
  1. `model-family-authoritative-projection`：新增只修改 provider 受管 model-family、保留 Profile 扩展的纯投影原语；有效配置采用 Common Config 最终优先，缺失/空模型删除。
  2. `provider-save-managed-profile-fanout`：在交互式 provider save 中同时覆盖 disabled direct 与 enabled routed Home，保持 catalog/runtime/route/补偿合同；覆盖多主引用、External、failover-only、disable 后保持新模型。
  3. `route-aware-explicit-sync-and-restore`：手动 Sync、Import/云恢复按每个 Managed Profile primary best-effort 扇出，聚合脱敏 warning；External 跳过。
  4. `document-model-family-convergence-contract`：更新 ADR/领域文档，明确普通 startup 保留 model-family、交互式 save 原子、Sync/Import best-effort 以及无 journal 的崩溃残留语义。
- Blocking edges：Issue 1 阻塞 Issue 2/3；Issue 2 与 Issue 3 可并行；Issue 4 以最终代码/测试合同为输入，阻塞最终一致性审查。
- 最高层测试 seams：`commands/provider.rs::update_provider_for_app` 的真实 DB + 临时 Home；`CodexRouteManager::update_shared_provider` 的 enabled/disabled/mixed reference 高层测试；新增 route-aware explicit sync manager seam，由现有 Sync/Import 命令薄委托。
- Requirement 覆盖矩阵：REQ-02～REQ-05 → Issue 01；REQ-01、REQ-06～REQ-09、REQ-14、REQ-15、REQ-17 → Issue 02；REQ-09、REQ-11～REQ-13、REQ-15、REQ-16 → Issue 03；REQ-01～REQ-17 的文档与最终证据 → Issue 04。REQ-10 的启动保留合同由 Issue 04 的既有回归验证封口。
- Scenario 覆盖矩阵：SCN-03～SCN-05、SCN-17 → Issue 01；SCN-01、SCN-02、SCN-06～SCN-10、SCN-16 → Issue 02；SCN-12～SCN-16、SCN-18 → Issue 03；SCN-01～SCN-18 的最终证据与 SCN-11 启动保留回归 → Issue 04。
- 粒度自审：Issue 01 是可独立验证的 prefactor/behavior primitive；Issue 02 与 03 各自贯穿高层 public seam 并可独立演示；Issue 04 只在实现完成后固化合同与完成审计。没有 wide refactor。
- Blocking graph 为 `01 → {02,03} → 04`，无环；每个 issue 适合 fresh context，所有 requirements/scenarios 至少有一个实施或封口 issue 覆盖。

---

*每两次重要查看、搜索、实验后更新本文件。*

## Execution Context

- Review Base Commit: `4d5af755`
