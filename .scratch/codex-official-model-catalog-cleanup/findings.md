# Findings & Decisions

## Requirements

- 诊断从 Codex 官方配置切换到第三方配置，再切回官方配置后，`~/.codex/config.toml` 仍残留 `model_catalog_json = "cc-switch-model-catalog.json"` 的问题。
- 证明残留配置为何会让官方订阅继续使用 cc-switch 自定义模型目录。
- 当前阶段只诊断，不修改业务代码；根因经用户确认后再进入修复设计。

## Findings

- 当前检出为 `/Users/qihoo/Documents/A_Code/Fork/cc-switch`，分支 `feature/v3.19.2`，开始诊断时工作树无未提交改动。
- 历史实现契约要求 provider 保存、切换、手动同步、导入和恢复通过同一套权威 model-family 投影收敛；历史修复重点是 provider 保存后的 `base_url`，本次仍需独立验证官方切换时的 `model_catalog_json` 删除语义。
- 历史诊断曾证明旧版 catalog-only 投影会保留 Home 既有配置，而 direct provider plan 会完整写入 provider 配置；这些仅作为待验证线索，不作为当前根因结论。
- 现有 Home-config 单元测试 `provider_without_catalog_keeps_old_catalog_file` 已覆盖浅层删除能力：无 catalog 的 provider 可移除 cc-switch 自有 `model_catalog_json` 指针并保留其他配置。这说明“底层完全不会删除该字段”不是充分解释，必须在真实 provider 切换 call site 建立复现。
- 当前搜索到多处官方 provider 集成测试，但尚未看到“第三方带 catalog → 切回官方 → 断言最终 live config 无 `model_catalog_json`”这一完整用户路径的断言；最接近的候选 seam 是 `src-tauri/tests/provider_service.rs` 的官方切换测试。
- `provider_service_switch_codex_official_clears_stale_third_party_auth` 已经构造“当前为第三方、live config 为第三方、切到 material-less 官方 provider”的真实 switch seam，但只断言清理 `auth.json`/`experimental_bearer_token`，没有给输入加入 cc-switch catalog 指针，也没有断言该指针被清理。
- 该 seam 可以通过仅增加一个前置字段和一个最终断言来形成快速、确定、无人值守的反馈环；前置的“官方 → 第三方”步骤可被最小化为其最终必要状态（第三方为 current 且 live config 含 cc-switch 指针），因为待验证的错误动作只发生在“第三方 → 官方”切换。
- 上述最小化假设已被实验推翻：加入 cc-switch 指针并执行真实“第三方 → 官方”switch 后，focused test 通过，最终配置不含 `model_catalog_json`。因此“官方 → 第三方”不是可安全删除的前置，它可能通过 outgoing-provider backfill/持久化改变随后被切回的官方 provider 数据。
- 第二次测试运行明确执行 `running 1 test`，结果 `1 passed`，运行测试本体耗时 0.05 秒；编译缓存建立后 seam 足够快且稳定，但尚未 red-capable。
- 完整“官方 → 带 modelCatalog 的第三方 → 同一官方”focused integration test 也通过：`running 1 test`、`1 passed`，测试本体 0.06 秒；通用 `ProviderService::switch` round trip 会删除指针。
- 2026-08-18 当前只读检查 `/Users/qihoo/.codex/config.toml`：包含 `model_provider = "custom"`、`model = "gpt-5.6-sol"` 和 `[model_providers.custom]`，但当前未匹配到顶层 `model_catalog_json`；同目录的 `cc-switch-model-catalog.json` 文件仍存在并列出第三方模型。文件存在本身不会让 Codex 使用它，必须由配置指针引用。
- 当前真实状态与用户刚才观察到的“指针残留”不完全一致，可能已被后续动作改写；需要从 cc-switch DB 当前选择和日志/持久化 provider 结构还原当时路径，不能把静态现状当成 Bug 未发生。
- 真实 DB 的 Codex provider 列表里同时存在两个名为 `OpenAI Official` 的条目：全局 `is_current = 1` 是 UUID `9c15dc2e-...`（`category = official`、配置非空），另一个固定 ID `codex-official`（`category = official`、配置为空）不是全局 current。
- 默认 Codex Profile `codex-default` 指向 `/Users/qihoo/.codex`，其 route 的 `current_provider_id = codex-official`、`enabled = 0`、`home_ownership = managed`；因此默认 Home 的 Profile 路由选择与全局 provider current 并非同一条记录。
- 当前全局 current 官方记录和固定 `codex-official` 在 DB 中都没有存储 `model_catalog_json`/modelCatalog；但真实 live config 仍为 custom provider 配置。问题至少涉及“选择状态与最终 Home 配置没有收敛”，不能只检查 catalog 文件生成函数。
- 当前公开 Tauri `switch_provider` 命令对 `AppType::Codex` 明确返回“Codex 供应商切换必须指定 Codex Profile”，因此先前 `ProviderService::switch` 绿色测试不覆盖当前 UI 的真实 Codex 切换路径。
- Codex UI 的实际命令 `switch_codex_profile_provider` 先从 DB 取 provider、合并 common config，再调用 `CodexRouteManager::switch_provider_with_effective_settings*`。默认 Profile route 为 disabled 时，真实 seam 是关闭态 Profile 的直连切换。
- 现有 route-manager 测试 `switching_disabled_profile_preserves_route_state` 覆盖关闭态切到官方 provider，但没有给旧 Home 配置加入 `model_catalog_json`，也没有断言该字段被删除；这是当前最接近用户路径的 correct seam。
- 在该 route-manager seam 中加入旧 Home 的 cc-switch catalog 指针后，focused lib test 首次有效运行变红：`running 1 test`，最终配置仍含 `model_catalog_json = "cc-switch-model-catalog.json"`，同时已写入 `model = "gpt-official"`。这证明关闭态 Profile 的官方切换真实执行了，但只完成部分配置投影，没有清理旧指针。
- 红色 test body 耗时 0.02 秒；首次 lib test 编译耗时 51 秒，后续可直接运行已编译 test binary 收紧反馈环。
- 直接运行已编译 test binary（完整测试名 + `--exact`）再次执行 `running 1 test` 并以相同残留指针失败，test body 0.01 秒；反馈环确定且 agent-runnable。
- 最小化实验删除旧 Home 的 `model_provider = "custom"`，只保留 `model_catalog_json` 一个输入字段，测试仍以同一症状失败；旧 model-provider 配置不是复现所必需。
- 最小化实验删除 official provider 的 `auth_mode = chatgpt`，只保留空 auth，测试仍失败；认证内容不是复现所必需。
- 继续把 official provider 的 `config` 缩减为空字符串（与固定 `codex-official` seed 一致），测试仍失败，实际 Home 仍含 catalog pointer。route DB 已更新为 official，足以证明切换动作执行；official provider 不需要任何配置 material 即可复现。
- 删除所有 failover provider/引用后测试仍失败；failover 队列不是必要条件。

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| 优先在现有 Rust 测试 seam 建立红色复现 | 该问题是配置投影/删除语义，函数或集成测试应能在数秒内稳定断言最终 `config.toml` |
| 不直接检查或修改用户真实 `~/.codex/config.toml` | 先通过隔离临时目录复现，避免改变当前 Codex 运行环境；必要时再只读核对真实文件 |
| 根因确认前不修改业务代码 | 遵循 diagnosing-bugs 的根因确认门禁 |
| 将最小红色复现拆成新的独立测试，不弱化原 `switching_disabled_profile_preserves_route_state` | 原测试承担 failover/route-state 覆盖；最小 Bug 复现应单一职责并保留既有断言 |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| 第一次运行 focused Cargo test 在 `objc2-exception-helper` 编译阶段失败：`cc` 输出“检测运行中的代理”并报 `unknown option '-MD'`，未执行任何测试 | 不重复原命令；先只读确认 PATH 中 `cc`/`ar` 的实际解析，再显式指定 Apple Clang 与系统 archiver 运行 |
| 最小化为“第三方 live 状态 → 官方”的测试是绿色，未复现用户现象 | 判定最小化删除了必要前置；改为完整执行“官方 → 第三方 → 官方”，观察官方 provider 在首次切出时的持久化变化 |
| 完整三步 integration test 仍为绿色 | 不再重复相同 provider fixture；转而只读采集真实 provider 元数据、当前选择和相关日志，再用真实形态构造差异化复现 |
| 前两个绿色测试调用了内部 `ProviderService::switch`，而公开命令已禁止 Codex 走该路径 | 放弃该 loop；转到 `switch_codex_profile_provider` → `CodexRouteManager` 的关闭态 Profile 测试 seam |
| 更新 findings 的补丁因章节顺序与上下文假设不一致而未应用 | 先读取完整相关段落，再用独立窄锚点更新；未重复失败补丁 |
| 移除实验性 catalog pre-call 的首次补丁上下文未匹配 | 读取目标测试的精确片段后，仅删除刚加入的 6 行；没有重复失败补丁 |
| 同时更新 findings 两个章节的补丁再次因跨章节上下文未匹配 | 改为先用 `rg -n -C` 定位两个窄锚点，再成功单点插入；后续不再使用跨章节大补丁 |
| `cargo fmt --check` 首次发现新测试函数签名换行不符合 rustfmt | 按检查器给出的唯一格式用 `apply_patch` 修正；复跑 `cargo fmt --check` 与 `git diff --check` 均通过 |

## Resources

- `.scratch/codex-official-model-catalog-cleanup/findings.md`
- `/Users/qihoo/.codex/memories/MEMORY.md` 中的 cc-switch authoritative Codex Profile provider-save synchronization 记录
- `/Users/qihoo/.agents/skills/diagnosing-bugs/SKILL.md`

## Visual/Browser Findings

- 本问题尚不需要视觉或浏览器证据。

## 反馈环

- 状态：真实 route-manager seam 已满足 red-capable、deterministic、fast、agent-runnable；当前继续最小化 official provider fixture。
- 首次尝试：`cargo test --manifest-path src-tauri/Cargo.toml --test provider_service provider_service_switch_codex_official_clears_stale_third_party_auth -- --nocapture`。
- 首次结果：exit 101，`objc2-exception-helper` 报 `unknown option '-MD'`，输出表明 PATH 中的 `cc` 是代理工具；测试数量为 0，不能作为 Bug 证据。
- 第二次尝试：显式设置 Apple Clang 和 Xcode `ar` 后运行同一 focused test。
- 第二次结果：`running 1 test`，`1 passed`；最终 live config 已删除 `model_catalog_json`，说明单次切回不是用户报告的 failure mode。
- 第三次尝试：新增完整 round-trip integration test，显式执行官方 → 第三方（确认指针生成）→ 同一官方（断言指针删除）。
- 第三次结果：`running 1 test`，`1 passed`；当前通用 switch 路径与普通 `category = official` fixture 无法复现。
- 第四次尝试：在 `switching_disabled_profile_preserves_route_state` 中为关闭态 Profile Home 写入旧 cc-switch catalog 指针，再通过 Route Manager 切到官方。
- 第四次结果：`running 1 test`，断言失败；实际配置仍含 `model_catalog_json`，精确捕获用户症状。命令为 `env CC=<Apple Clang> AR=<Xcode ar> cargo test --manifest-path src-tauri/Cargo.toml --lib switching_disabled_profile_preserves_route_state -- --nocapture`。
- 确定性复跑命令：`src-tauri/target/debug/deps/cc_switch_lib-0a745617cbba836b 'codex_profile::route_manager::codex_route_manager::switching_disabled_profile_preserves_route_state' --exact --nocapture`；`running 1 test`，相同断言失败，0.01 秒。
- 最终反馈环已拆为独立测试：`env CC=<Apple Clang> AR=<Xcode ar> cargo test --manifest-path src-tauri/Cargo.toml --lib switching_disabled_profile_to_official_clears_catalog_pointer -- --nocapture`；`running 1 test`，相同残留失败，0.02 秒。
- 原有 `switching_disabled_profile_preserves_route_state` 已逐字恢复其 fixture/断言，并用已编译二进制精确运行通过（`running 1 test`、`1 passed`、0.01 秒），未弱化既有 route/failover 覆盖。
- 目标症状：先应用带 `model_catalog_json` 的第三方配置，再切回不应带该字段的官方配置，最终落盘配置仍包含该字段时测试必须失败。

## 最小复现

- 第一次候选最小化（直接构造第三方 current/live + catalog pointer，再切官方）过度缩减并变绿。
- 当前保留的必要候选步骤：官方 current/live → 切第三方并确认生成 catalog pointer → 切回同一个官方 provider → 断言 pointer 被移除。
- 完整候选步骤在普通官方 fixture 下为绿色；剩余最小化工作转为识别真实官方 provider 与测试 fixture 的结构差异，以及 UI 是否走另一条命令链。
- 最终最小复现：一个关闭态 Managed Codex Profile，其 Home `config.toml` 只含 `model_catalog_json = "cc-switch-model-catalog.json"`；一个 `category = official`、空 auth、空 config 的 provider；调用 Route Manager 的关闭态 Profile 切换。无需旧 model_provider、catalog 文件、认证 material、provider config 或 failover。
- 删除唯一的 catalog pointer 会使精确断言自然变绿且不再存在用户症状；删除 Profile switch 则不会执行目标 call site。因此剩余两个行为元素均不可移除。
- 已识别真实 UI seam：关闭态默认 Profile 通过 `switch_codex_profile_provider` 调用 Route Manager；下一步给现有关闭态官方切换测试加入旧 catalog pointer 并断言删除。
- 当前最小输入已缩减为：关闭态 Managed Profile Home 仅含 cc-switch catalog pointer + 一个 `category = official`、空 auth、空 config 的 provider + Profile 切换调用。正在移除无关 failover fixture。

## Hypotheses 与实验

- H1（最高）：关闭态直连计划把“目标 provider 没有 modelCatalog”解释为“不触碰现有目录指针”，而正确语义应是“删除 cc-switch 自有指针”。预测：计划对象对 `model_catalog_json` 没有删除 mutation；显式空 catalog/单独 catalog projection 会改变结果。
- H2：关闭态 Profile 切换的调用顺序只应用 direct-provider plan，完全没有调用已有的 model-catalog projection 清理路径。预测：从 Route Manager 的 disabled 分支到 apply 的调用链中找不到 catalog projection；非官方但无 catalog 的 provider 也会残留，说明 category 不是决定因素。
- H3：删除依赖 catalog 文件存在或 ownership 校验；当前最小 fixture 只有指针没有文件，导致守卫把它当作用户目录保留。预测：先创建合法的 `cc-switch-model-catalog.json` 文件后，同一切换会变绿。
- H4：计划曾删除指针，但后续对旧 Home 扩展字段的 merge 又把它加回来。预测：对 plan build/apply 边界做单变量观察，会看到中间结果无指针而最终结果恢复；若 build 阶段结果已含指针则 H4 被否证。
- 静态调用链已支持 H2 的前半：disabled direct 分支确实不调用独立 catalog projection。H1/H4 仍需观察 direct plan 构建结果，H2 的“category 非必要”仍需实验，H3 仍需补 catalog 文件实验。
- H3 已否证：在相同 Home 内补建合法 `cc-switch-model-catalog.json` 文件后，指针仍以相同方式残留；文件是否存在不影响失败。
- H2 的 category 预测已确认：把目标 provider 的 `category` 单独改为 `None` 后仍残留指针，且最终配置只剩该指针；因此缺陷属于所有“无 modelCatalog 的关闭态直连切换”，官方只是用户最危险、最直观的触发目标。
- H1 已由实现链确认：`prepare_codex_config_with_model_catalog` 的删除能力只作用于传入的 `config_text`；直连计划传入目标 provider 的配置（固定 official seed 为空），并未传入当前 Home，因此它在错误的数据源上执行了“删除”。
- H4 的“apply 后重加”已否证：`merge_authoritative_direct_provider_projection` 在 plan build 阶段先以当前 Home 为基底，再只合入 projected config 明确声明的字段；`model_catalog_json` 不属于 model family，且 projected official config 没声明它，所以计划目标本身已经保留旧指针。没有证据表明 apply 后另有写回。
- 现有专用 `prepare_codex_model_catalog_projection` 恰好采用正确的两个输入：provider config 用于生成 catalog，Home config 用于注入/删除指针；现有 `empty_catalog_removes_only_cc_switch_pointer` 测试证明它会删除 cc-switch 指针并保留 Home 的 model。
- 单变量确认通过：在同一 route-manager 测试、同一 official provider、同一 Home 前置状态下，只在 switch 前显式执行现有 `build_model_catalog_projection_plan` + `apply_model_catalog_projection_plan`，测试从红转绿（`running 1 test`、`1 passed`、0.02 秒）。移除该 pre-call 后又恢复为相同红色残留。由此确认缺失的 Home catalog projection 是必要且足以解释症状的差异。
- Ownership 边界现有测试通过：`empty_catalog_removes_only_cc_switch_pointer` 与 `empty_catalog_preserves_user_managed_pointer` 共 `2 passed`。因此已有底层语义能够删除 cc-switch 自有相对指针，同时保留用户自定义外部 catalog 路径；本 Bug 不需要粗暴删除所有 `model_catalog_json`。
- 对照路径测试 `provider_service_codex_round_trip_control_clears_model_catalog_pointer` 通过（`running 1 test`、`1 passed`、0.06 秒），进一步证明旧单 Home `ProviderService` 往返路径能清理，缺陷专属于当前公开 Profile Route Manager 的 disabled direct 分支。

## 调用链与 Data Flow

- UI command `switch_codex_profile_provider`：读取 DB provider → `build_effective_settings_with_common_config` → `CodexRouteManager::switch_provider_with_effective_settings*`。
- `switch_provider_internal`：读取 Profile/route；当 `route.enabled == false` 时进入 `switch_direct_provider_locked`。
- `switch_direct_provider_locked`：调用 `build_authoritative_direct_provider_plan` → `apply_direct_provider_plan` → 持久化 route/failovers。
- `build_authoritative_direct_provider_plan` 进入 `build_direct_provider_plan_with_mode(...ApplyAuthoritativeModelFamily)`：先准备 provider config/catalog，再读取当前 Home，最后调用 `merge_authoritative_direct_provider_projection(current, prepared.config_text)`。
- 与之对照，`build_model_catalog_projection_plan_from_snapshot` 会基于当前 Home 调用 `prepare_codex_model_catalog_projection`，可生成专门的 config 删除计划；disabled direct 分支没有调用该 projection API。
- `apply_direct_provider_plan` 只在 `prepared.model_catalog` 为 `Some` 时写 catalog 文件，然后原样应用 direct config plan；它没有第二次 catalog 指针清理。
- `prepare_codex_config_with_model_catalog(settings, provider_config, ...)` 在无 modelCatalog 时会调用 `set_codex_model_catalog_json_field(provider_config, None)`；official seed 的 `provider_config == ""`，所以返回仍为空。随后 authoritative merge 以当前 Home 为基底并保留未被 projected config 声明的字段，旧指针进入最终 plan target。

## 根因

- 技术根因已由红/绿差分确认，等待用户产品层确认后进入修复设计。
- 最早错误来源：关闭态 Profile 的显式切换只构造 authoritative direct-provider plan。该计划在目标 provider 自己的配置文本上删除 catalog 指针，随后把结果合入当前 Home；由于 official seed 配置为空、当前 Home 才持有旧指针，而 merge 又保留目标未声明的扩展字段，最终 plan 从构造时就携带旧 `model_catalog_json`。
- 缺失机制：disabled direct switch 没有像 routed provider plan / catalog reconciliation 那样，基于当前 Home 运行 model-catalog projection。现有 projection 能按 ownership 规则只删除 cc-switch 自有指针并保留用户自定义外部 pointer。

## 未验证边界

- 用户报告时刻的 `~/.codex/config.toml` 原始快照未捕获；本次只读检查时该字段已不在文件中，因此不能用当前静态文件证明当时持续时长。
- 真实 DB 同时存在两个不同 ID 的 official provider；默认 Profile route 当前指向固定 `codex-official`，但未从历史 UI 日志确认用户当时点选的是固定 ID 还是同名 UUID。两者均为空/无 catalog 时会落入已复现的同一缺陷机制。
- 尚未用开发版 Tauri UI 做点击级 E2E；不过公开命令、Route Manager call site、最小红测和红/绿差分已覆盖决定最终 Home 文件内容的真实后端路径。

## Grilling 设计阶段

- 用户在根因诊断交付后显式调用 `$grilling`，当前视为已通过根因确认门禁；本阶段只固化修复设计、spec 与 issues，不提前修改业务实现。
- 外部知识缺口已核验：OpenAI 当前配置参考说明用户级配置位于 `~/.codex/config.toml`，`model_catalog_json` 是 Codex 启动时加载的可选模型目录 JSON 路径。因此残留指针不是无效元数据，而会改变官方订阅启动后可见的模型目录。
- 外部文档不规定 cc-switch 对自有目录指针的 ownership、补偿和启动收敛策略；这些必须由本仓库既有实现契约与用户产品决策确定，不能从 OpenAI 文档臆推。
- 已确认 `build_authoritative_direct_provider_plan` 至少服务于三条关闭态 Managed Profile 生命周期：显式 provider 切换、显式 Profile 同步、共享 provider 保存后的 Home 投影。
- 启动恢复对关闭态 Managed Profile 使用 `build_automatic_direct_provider_plan`；它与 authoritative 计划共享 direct-plan builder，但仅在 model-family 模式上不同。因此若只在 Route Manager 的“显式切换”调用点补一次 catalog 清理，手动同步、provider 保存与启动恢复仍可能保留相同陈旧派生状态。
- 启用态 Profile 走 routed plan；启动时在不满足关闭态直连条件时走独立 catalog projection，不属于本次已复现的缺口。

## 开放决策

- D1 已确认：按同一根因覆盖所有关闭态 Managed Profile 的 direct-plan 收敛入口，不只修复“关闭态显式切回官方”。

## 用户回答记录

- Q1 修复范围：用户选择方案 1“全链路统一修复”。
- 由此覆盖：显式 provider 选择/重选、显式 Sync 与 Import/Restore 后置同步、共享 provider 保存、启动 automatic reconcile。
- 由此不引入 official/category 特判；判定依据始终是目标 provider 是否提供有效 `modelCatalog` 以及当前 pointer 的 ownership。

## 已收敛约束与工程决策

- D2 已由现有 ownership 契约固定：只移除 basename 精确为 `cc-switch-model-catalog.json` 的 cc-switch 自有指针，保留用户管理的其他 catalog 路径。边界是：用户若手工使用同名文件，也会被当前契约视为 cc-switch-owned；本 Bug 不扩大该既有边界。
- D3 已由现有测试固定：切到无 catalog 的 provider 后只解除配置引用，不删除磁盘上的旧 `cc-switch-model-catalog.json`，以便后续复用。
- D4 已由原子性与补偿审计收敛：在 direct plan 构造期基于同一份 Home 快照组合 catalog projection 与 mode-specific provider projection，最终只生成一个 config target。不得在 Route Manager 做两个连续写入。
- D5 的最低回归矩阵由共享构造器调用关系决定：显式切换、显式 Sync/Import/Restore 后置同步、共享 provider 保存、启动 automatic reconcile；另加用户自有 pointer、外部并发冲突、DB 持久化失败恢复。UI 点击层不包含额外业务分支，后端公开命令到 Route Manager 的 seam 已有静态证据，可在实现完成审计时再判断是否需要烟测。

## 外部方案比较

- 方案 A（窄修）：仅在 `switch_direct_provider_locked` 前后追加 catalog projection。优点是 diff 小；缺点是形成两次独立 Home 写入，第二次或 route 持久化失败时补偿边界更复杂，并遗漏其他 authoritative/automatic direct-plan 调用者。
- 方案 B（统一 direct plan，当前技术推荐）：direct-plan 构造阶段同时使用 provider config 与同一份 Home snapshot 完成 catalog projection，再合并 mode-specific model-family；一次计划携带 catalog 文件与最终 config，复用既有 apply/restore 原子边界。优点是所有 direct-plan 调用者一致收敛，缺点是影响面更广，必须补齐 automatic 模式与用户自有 pointer 的回归测试。
- 方案 C（粗暴字段删除）：在 official/category 分支无条件删除 `model_catalog_json`。优点是实现最短；缺点是错误依赖 provider 类别，并会破坏用户手工配置的 catalog 路径，已与现有 ownership 测试契约冲突。

## 调用方审计补充

- `build_direct_provider_plan` 当前没有生产调用者，只存在于 `#[cfg(test)]`；真实显式切换已经走 authoritative builder。
- `build_authoritative_direct_provider_plan` 的生产入口包括：关闭态显式 provider 选择/重选、手动显式 Sync（并被 Import/Restore 后置同步复用）、共享 provider 保存对关闭态 Managed 主 Home 的投影。
- `build_automatic_direct_provider_plan` 的唯一生产入口是应用启动时对关闭态 Managed Profile 进行数据库权威的 derived-state reconcile。
- automatic 模式只保留用户当前 `model` 与 `model_reasoning_*`；`model_catalog_json` 是供应商派生状态，不是用户模型选择，仍应按数据库当前 provider 收敛。
- routed builder 已提供可直接镜像的正确先例：只读取一次 Home 快照，先在该快照上做 ownership-aware catalog projection，再生成最终配置计划；direct builder 应复用相同组合方式，同时保留 authoritative 与 automatic 的 model-family 差异。

## 原子性、并发与补偿审计

- 推荐方案在无 catalog 的 official 切换中只进行一次 config 原子替换；有 catalog 时沿用现有“先写辅助目录文件、再写 config，config 失败则恢复目录”的补偿式原子性，不宣称两个文件具备 OS 事务。
- direct config plan 应继续以原始 Home 快照 H0 作为 previous，并以组合后的唯一配置作为 target；apply 前的 fingerprint 检查可拒绝构建计划后的外部编辑。
- 后续 route/failover DB 持久化失败时，现有恢复链能从 target 回到 H0；如果拆成 H0→H1→H2 两次 Home 写入，当前恢复链最多回到 H1，无法完整补偿。
- 补偿仍需遵守“仅当当前内容等于本计划 target 时恢复”的冲突保护，避免覆盖并发外部编辑。

## 被拒绝方案

- 不按 `category = official` 特判：实验已证明任何无 modelCatalog 的关闭态 provider 都能触发，类别不是根因。
- 不删除所有 `model_catalog_json`：必须保留用户自有外部 catalog 路径。
- 不以删除旧 catalog JSON 文件替代清理指针：Codex 是否加载目录由配置引用决定，文件是否孤立存在不是本次故障条件。

## Spec/Issue 覆盖自审

- 领域词汇采用 `CONTEXT.md`：共享供应商、Profile 主供应商引用、Profile 直连配置、Profile 派生状态、有效模型族、Profile 自有扩展、明确 Profile 同步。
- 设计与已接受 ADR-0001 一致：共享供应商是 Managed Home 派生状态的权威来源；明确同步权威更新有效模型族，启动对账保留 Home 当前 model family；两者都应收敛不属于 model family 的供应商派生目录指针。
- 已确认 test seams：Route Manager 的关闭态显式切换 public seam 捕获原始用户故障；Home Config Service 的 direct plan public seam锁定 ownership、单快照与并发保护；Route Manager 的显式同步、共享供应商保存和启动对账 public seams 锁定其他 lifecycle。
- Stage 5 必须保留并先运行当前最小红测，修复后转绿，再用“带 catalog 第三方 → 无 catalog 官方”的完整往返场景复验；不能用旧 `ProviderService` 绿色对照代替真实 Profile seam。
- Stage 6 必须重跑原始 repro 与 regression matrix，搜索并确认不存在 `[DEBUG-...]`，删除旧 seam 的 throwaway 诊断测试，并在 issue commit message 中记录被证实的 hypothesis。
- Vertical issues 计划：01 交付显式直连切换与共享 direct-plan 组合修复；02 交付明确同步/共享供应商保存的全链路回归；03 交付启动对账、并发/补偿与 Stage 6 完成审计。02、03 均以 01 为 blocker；协调器因测试文件冲突顺序执行。
- requirements/scenarios 与 issues 的完整覆盖矩阵见下表；blocking graph 已确认为 `01 → {02,03}`，无环。

### 覆盖矩阵

| Issue | Requirements | Scenarios | Test seams | Blocking |
|---|---|---|---|---|
| 01 统一直连切换的模型目录投影 | REQ-01、REQ-05、REQ-06、REQ-07、REQ-09、REQ-10 | SCN-01、SCN-02、SCN-03、SCN-04、SCN-08、SCN-09 | TS-01、TS-02 | 无 |
| 02 覆盖明确同步与供应商保存的目录收敛 | REQ-02、REQ-03、REQ-05、REQ-06、REQ-07、REQ-08、REQ-09、REQ-10 | SCN-03、SCN-04、SCN-05、SCN-06、SCN-08、SCN-10、SCN-11 | TS-03、TS-04 | 01 |
| 03 验证启动对账与失败安全 | REQ-01、REQ-04、REQ-06、REQ-07、REQ-08、REQ-09、REQ-10 | SCN-03、SCN-07、SCN-08、SCN-09、SCN-10、SCN-11、SCN-12 | TS-01、TS-02、TS-05 | 01 |

### 自审结果

- spec 中 REQ-01 至 REQ-10、SCN-01 至 SCN-12、TS-01 至 TS-05 均至少由一个 issue 覆盖，没有孤立条款。
- issue 01 是可独立演示的最小 vertical slice：真实显式切换从 red 到 green，并通过共享 direct plan 交付 ownership 与并发行为。
- issue 02、03 都只依赖 issue 01 的统一 direct-plan contract，彼此不存在语义 blocker；考虑到它们会修改相同 Route Manager 测试模块，协调器顺序调度以降低冲突风险。
- blocking graph 为 `01 → 02` 与 `01 → 03`，无环；不存在尚未解除 blocker 的 issue 被提前调度。
- diagnosing Stage 5 由 issue 01 的最小 red/green 和 issue 03 的完整原始往返复验覆盖；Stage 6 由 issue 03 的 regression、instrumentation 搜索、throwaway 清理与最终验证覆盖。
- spec 没有扩大到 routed Profile、ownership 重定义、旧目录文件删除或持久化事务日志，和用户确认范围一致。

## 资料来源

- OpenAI Codex 配置参考：`https://developers.openai.com/codex/config-reference/`（当前会重定向到 Learn ChatGPT 文档；已核对 `config.toml` 与 `model_catalog_json` 的字段语义）。

## Execution Context

- Review Base Commit: `d6e13d43b64a16930dfaed9cda4890dd05b192e2`

## Issue 02 执行审计

- Blocker 01 已由提交 `01153d8f0a255a9283b211221983be15ea1a8d2b` 完成；该提交把目录 ownership 投影并入 authoritative/automatic 共享 direct plan。当前 HEAD 是其后续文档提交，findings 中只存在一个 Review Base，且 `d6e13d43b64a16930dfaed9cda4890dd05b192e2` 是当前 HEAD 的祖先。
- TS-03 的公开 seam 是 `CodexRouteManager::sync_managed_profiles_explicit`；关闭态 Managed 分支经 `sync_disabled_profile_explicit` 直接消费 `build_authoritative_direct_provider_plan`，因此无需新增目录清理生产分支。
- TS-04 的公开 seam 是 `CodexRouteManager::update_shared_provider`；其关闭态 Managed 主引用同样消费 `build_authoritative_direct_provider_plan`，External 主引用被跳过，纯故障转移引用不产生 Home plan。
- Import、数据库 Restore、S3 Restore 与 WebDAV Restore 都调用 `commands::sync_support::run_post_import_sync`；该函数最后调用同一个 `sync_managed_profiles_explicit`。因此这些入口以明确调用关系复用 TS-03，不应复制目录清理逻辑或新增平行测试 seam。
- Issue 02 的保留测试将从两个 public Route Manager seam 观察最终 Home、结构化同步结果和供应商保存结果；既有 best-effort warning、External、故障转移、批量写失败、数据库提交失败和敏感信息脱敏测试继续作为回归证据。
- Issue 02 首次组合回归编译在数据库失败注入闭包处停止：`fs::read_to_string` 的 `std::io::Error` 不能通过 `?` 自动转换为 `AppError`，测试数为 0。下一步改为在该测试系统边界显式映射为不含配置正文的 `AppError::Config`，不会重复未经修正的命令。
- Issue 02 的显式 Sync 新测试在 Review Base 负控上实际运行 1 项并因旧 `model_catalog_json` 残留失败；当前共享修复上实际运行 1 项并通过，同时证明有效模型族更新、Profile 扩展保留且 warning 为空。
- Issue 02 的共享供应商保存新测试在 Review Base 负控上实际运行 1 项并因 Managed 主 Home 的旧指针残留失败；当前共享修复上实际运行 1 项并通过，同时证明两个 Managed 主引用均收敛、External Home 与纯故障转移引用 Home 完全不变。
- 编译失败已通过显式映射测试文件读取错误修复；随后显式 Sync 测试组 4 项通过，共享供应商保存测试组 16 项通过，Import/Restore 统一结果测试 2 项通过，Route Manager 全部 146 项通过。
- 供应商保存补偿 fixture 已改为“旧供应商有目录、待保存供应商无目录”：批量第二个 Home 写失败会恢复第一个 Home，数据库提交失败测试在 commit 闭包内确认指针已经清理后再注入失败，并验证所有 Home、目录文件、供应商记录和 runtime snapshot 恢复且错误不泄露 token。

## Issue 03 执行审计

- Blocker 01 已完成，findings 中只有一个 Review Base `d6e13d43b64a16930dfaed9cda4890dd05b192e2`，且它是执行时 HEAD 的祖先；issue 03 没有新增生产分支，只补齐启动、失败安全和原始往返的 public-seam 回归。
- TS-05 启动测试在临时恢复 Review Base direct-plan 行为后实际运行 1 项并因 owned 目录指针残留失败；恢复 issue 01 的共享投影后同一测试通过，并保留用户 `model`、`model_reasoning_effort`、`model_reasoning_summary`、Profile/Desktop 扩展和旧目录文件。
- 启动失败隔离与 External ownership 继续由既有 `reconcile_all_profile_derived_state_repairs_disabled_homes_idempotently_and_isolates_failure` 和 `external_takeover_skips_later_disabled_derived_state_projection` 覆盖，两项均包含在 Route Manager 149 项绿色回归中。
- TS-01 持久化失败测试在临时移除 `persist_disabled_provider_selection_locked` 的 Home 补偿后实际运行 1 项并因 Home 停留在投影后状态失败；恢复补偿后同一测试通过，切换前包含旧目录指针的完整 Home 逐字恢复。
- TS-02 条件恢复测试在临时移除 target fingerprint 检查后实际运行 1 项并因外部 Home 被旧快照覆盖而失败；恢复 CAS 检查后同一测试通过，补偿冲突返回错误且并发外部内容保持不变。
- Stage 5 完整关闭态往返测试在临时恢复 Review Base direct-plan 行为后实际运行 1 项并因官方阶段仍残留第三方目录指针失败；恢复共享投影后同一测试通过，第三方阶段相对指针和目录文件存在，官方阶段指针消失、主引用更新且旧目录文件保留。
- Stage 6 已让 `src-tauri/tests/provider_service.rs` 恢复到 HEAD，删除 diagnosing 阶段旧单 Home 绿色对照及扩展断言；搜索未发现 `[DEBUG-` 和旧对照测试名，未创建 prototype、临时 worktree 或依赖软链。
- 回归矩阵：issue 01/02/03 七项代表性 focused tests 全部通过；Route Manager 149 项、Home Config 47 项、Codex Config 81 项、Sync Support 2 项、ProviderService 36 项全部通过。
- 首次完整并发 Rust lib 运行 2662 passed、1 failed、5 ignored；唯一失败的数据库同步导入测试隔离运行通过，且本任务未修改对应文件，因此分类为并发基线噪声。改用串行线程后 lib 为 2663 passed、0 failed、5 ignored。
- 串行 workspace suite 随后停在既有 `provider_commands`：两个 Codex 测试仍调用当前明确禁止的通用 `switch_provider` 并稳定收到“必须指定 Codex Profile”，另外两个因测试互斥锁中毒级联失败的项目隔离运行通过。Review Base 至当前 HEAD 与 issue 03 均未修改该文件，本 issue 不扩展范围修复旧 seam。

## Issue 01 Review Follow-up

- Code review 发现 1 个 P3 Standards 问题：`build_direct_provider_plan_with_mode` 重复了 `build_model_catalog_projection_plan_from_snapshot` 已有的 Home snapshot UTF-8 解码与 ownership-aware catalog projection 编排，并为此将 `set_codex_model_catalog_json_field` 扩大为 `pub(crate)`。
- 处理方式：在 `CodexHomeConfigService` 内提取私有无落盘准备 helper `prepare_model_catalog_projection_from_snapshot`，仅负责从调用方已读取的 snapshot 解码 Home 并调用现有 `prepare_codex_model_catalog_projection`。catalog plan 与 direct plan 共用该 helper，低层 setter 恢复为 `codex_config.rs` 私有函数；未新增生产分支或改变公开 seam。
- 重构前护栏：`switching_disabled_profile_to_official_clears_catalog_pointer` 1 passed，`direct_provider` 过滤集 7 passed。重构后同样为 1 passed 与 7 passed，完整 `catalog` 过滤集 75 passed。
- 静态验证：`cargo fmt --all --check` 与 `git diff --check` 通过；`cargo clippy --lib` 退出 0，仅报告 `migration.rs`、`proxy/body_dump.rs` 与 `proxy/forwarder.rs` 中共 8 个既有 warning，本次修改文件无 Clippy 报告。
- 第一次静态命令在仓库根目录运行 `cargo fmt` 时因无 `Cargo.toml` 退出 1；未重复原命令，改为在 `src-tauri` crate 目录执行后通过。
- 第二轮 code review 指出新增 helper 注释中的英文表达不符合仓库“新增注释必须使用中文”的标准；已替换为“遵循所有权规则的模型目录投影”，只修正注释。

---

*每完成两次重要查看、搜索、实验或浏览后更新本文件。*
