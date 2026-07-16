# Codex 模型目录多 Home 调查记录

## 用户提供的第一手信息

- 旧行为：只有默认 `CODEX_HOME=~/.codex` 时，Codex 供应商配置只要涉及自定义模型，CC Switch 会生成 `cc-switch-model-catalog.json`，并由 `~/.codex/config.toml` 引用。
- 新现象：新增 `CODEX_HOME=~/.codex-api` 后，在该 Home 开启路由时，必须手动软链接 `/Users/qihoo/.codex/cc-switch-model-catalog.json` 才能使用同一份模型目录。
- 期望：供应商配置是全局共享的；某个 `CODEX_HOME` 中发生的相关更改，应能影响所有 `CODEX_HOME`。
- 生效边界：不要求运行中热更新；允许其他 Home 在下次重启 Codex 时读取最新目录。

## 调查目标

1. 追踪 `cc-switch-model-catalog.json` 的生成位置、生命周期和配置引用写入逻辑。
2. 确认当前实现是否把模型目录错误地绑定到了默认 `~/.codex`。
3. 对比多 Profile 路由启停和供应商切换时，对 `CODEX_HOME` 的处理是否一致。
4. 在不修改代码的前提下确认根本原因，之后再讨论“每 Home 副本”与“共享文件引用”等方案。

## 调查日志

- 2026-07-16：创建 findings，尚未修改任何实现。
- 全仓搜索确认当前已有两套相互关联的行为：
  - Codex 在启动时读取 `model_catalog_json`，因此目录更新后本来就需要重启；这与用户确认的生效边界一致。
  - `config.toml` 中保存的是相对文件名 `cc-switch-model-catalog.json`，Codex 会相对当前配置目录（即对应 `CODEX_HOME`）解析，而不是相对 CC Switch 的全局数据目录解析。
- 既有多 Profile 设计和测试明确把目录投影限定到当前 Profile Home：`src-tauri/src/codex_profile/home_config.rs` 包含 `direct_provider_model_catalog_is_scoped_to_profile_home` 测试；既有 spec 也写明“目录只写当前 Profile Home”。这说明当前缺失不是偶发写入失败，而是既定的 per-Home 投影策略与新增“全局共享目录内容”需求不一致。
- 旧版 changelog 和用户手册仍以 `~/.codex/cc-switch-model-catalog.json` 描述单 Home 行为；多 Home 后，相对路径指针会自然要求每个 Home 下都存在同名文件。
- `src-tauri/src/codex_config.rs` 仍保留默认 Home API：
  - `get_codex_model_catalog_path()` 通过 `get_codex_config_dir()` 返回默认 Home 下的目录路径。
  - `prepare_codex_config_with_model_catalog()` 是纯准备函数，但内部仍调用默认路径；该路径目前只用于决定是否向 TOML 注入相对文件名，实际注入值始终是常量 `cc-switch-model-catalog.json`。
  - `prepare_codex_config_text_with_model_catalog()` 明确标注并实现为“生成默认 Codex Home 的模型目录”。
- `resolve_cc_switch_catalog_path()` 对相对路径采用调用者传入的 `generated_path`，因此底层具备按不同 Home 解析投影文件的能力；问题不在 Codex/TOML 无法支持多 Home，而在上层投影的生成与同步范围。
- `CodexHomeConfigService::build_direct_provider_plan(home, provider)` 已经接收显式 Home，并把生成目录计划的路径设为 `<home>/cc-switch-model-catalog.json`；`apply_direct_provider_plan()` 先写该目录，再写同一 Home 的 `config.toml`，配置写失败时会补偿目录。
- 该直连事务一次只包含一个 Home 的 config 和一个 Home 的目录；没有“枚举所有 Profile 并刷新其目录”的步骤。
- 测试 `direct_provider_model_catalog_is_scoped_to_profile_home` 不仅遗漏了全局同步，还明确断言其他 Home **不得**生成目录。这是现行为的直接证据。
- 目录文件和 `config.toml` 被放在同一单 Home 补偿事务中，说明直接改成共享物理文件会改变现有的所有权/指纹/补偿边界；后续方案不能只替换一个路径而忽略事务语义。
- 调用链确认有两条不同路径：
  - 路由关闭态切换供应商：`switch_direct_provider_locked()` 调用 `build_direct_provider_plan()` / `apply_direct_provider_plan()`，会把目录写入当前 Profile Home。
  - 开启路由：`enable_internal()` 只调用 `build_profile_route_plan()`，该计划只管理当前 Home 的 `config.toml`，没有模型目录辅助文件计划。
- 因此用户在 `~/.codex-api` 开启路由时缺文件，并不是显式 Home 路径拼错；而是路由开启链路压根没有把当前供应商的共享 `modelCatalog` 投影到该 Home。若该 Home 此前没在关闭态直连切换过带目录的供应商，就不会自然存在文件。
- 启动恢复 `build_restore_plan()` 同样只重建路由 TOML 和运行时快照，不补建目录，所以应用重启也不能自愈缺失的 Profile 目录。
- `build_codex_profile_route_toml()` 只更新路由接管字段（`base_url`、`wire_api`、`experimental_bearer_token`，以及可选的 `model`）；它既不注入也不删除 `model_catalog_json`。因此：
  - Home 原来有目录指针时，开启路由会保留指针；
  - Home 原来没有目录指针时，开启路由不会创建指针；
  - 即使保留了指针，开启路由也不会确保同 Home 下的目标 JSON 存在或内容与当前共享供应商一致。
- 一次代码搜索误用了不存在的 `src-tauri/src/codex_profile/persistence.rs` 路径；后续改为先通过 `rg --files src-tauri/src/codex_profile` 定位真实持久化模块，不重复该失败路径。
- 已定位真实持久化实现为 `src-tauri/src/codex_profile/repository.rs`，Profile/路由模型位于 `model.rs`。
- 旧全局代理路径的 `attach_codex_model_catalog_from_provider()` 会把供应商数据库中的 `settings_config.modelCatalog` 附到代理 live settings；这进一步证明目录内容的权威源是共享供应商记录，而不是默认 Home 的 JSON 文件。
- 但“代理运行时拿到了 modelCatalog”与“Codex 启动时能从 Home 读取目录文件”是两个独立边界：前者可用于代理的模型服务，不能自动替代当前 Home 缺失的相对 JSON 文件。
- 每个 Profile 的 runtime snapshot 从共享数据库按 `current_provider_id` 读取同一个 `Provider`，然后复制进该 Profile 的独立监听器；请求上下文/熔断器是隔离的，但供应商定义本身全局共享。
- Profile repository 已能通过数据库 `list_codex_profiles()` 枚举所有已注册 Home，数据库也保存每个 Profile 的 `current_provider_id`。因此实现“找出引用某供应商的所有 Home”不缺基础数据，缺的是明确的同步服务和事务策略。
- 当前运行时支持 `swap_provider_snapshot()`，可在供应商切换时更新已运行监听器；这与磁盘目录刷新仍是两条链路。用户已允许磁盘目录在 Codex 下次重启时生效，因此不需要让 Codex 进程热重载文件。
- 当前机器状态核对（未读取任何凭证）：
  - `~/.codex/cc-switch-model-catalog.json` 与 `~/.codex-api/cc-switch-model-catalog.json` 当前都显示为普通文件，并非符号链接；两者大小相同但修改时间不同。用户此前的手工软链接可能已被后续原子写替换为普通文件，这与 `atomic_write` 的替换语义一致。
  - `~/.codex-api/config.toml` 当前包含相对指针 `model_catalog_json = "cc-switch-model-catalog.json"`；默认 `~/.codex/config.toml` 当前未检出该指针。
  - 数据库中默认 Profile 当前绑定供应商 `9c15...` 且路由关闭；`codex-api` Profile 绑定供应商 `4847...` 且路由开启。两个 Home 当前使用不同供应商，说明“所有 Home 永远指向同一份目录内容”会与每 Home 独立选供应商的既定需求冲突。
- 供应商 `4847...`（`claude-openai-chat`）在共享 DB 中有 8 条模型映射；默认 Profile 绑定的是无自定义目录的官方供应商。
- 两个 Home 当前的 JSON 都列出 `claude-openai-chat` 的开头模型，但 SHA-256 不同，说明它们不是同一个 inode/共享文件，也不是字节一致的同步副本；默认 Home 留有第三方供应商目录残留，但其当前官方配置没有目录指针，因此不会被 Codex 使用。
- 这验证了 JSON 是可滞留、可分叉的派生缓存。仅凭“文件存在”不能判断其是否对应 Profile 当前供应商；有效性取决于当前 Home 配置指针和当前供应商引用共同决定。
- 通用 `update_provider` 命令只委托 `ProviderService::update()`；多 Profile 切换则走独立的 `switch_codex_profile_provider` / route manager。两条生命周期目前不是同一个聚合事务。
- 数据库已经提供 `list_codex_provider_profile_refs()`，但现用法仅在删除供应商前阻止删除；供应商更新路径未见利用该引用列表刷新相关 Home 的目录投影。
- 旧的单 Home Codex 切换仍由 `ProviderService` / 全局 proxy service 维护默认 live 配置，这解释了为何默认 `~/.codex` 目录逻辑继续存在；新增 Profile route manager 是并行的新生命周期，尚未完整接管模型目录同步。
- Profile 供应商切换的分支已明确：
  - 路由关闭：写当前 Home 的直连 config + catalog。
  - 路由开启：只替换 runtime snapshot、保存 `current_provider_id` 与 failover；完全不触碰 Home 的 catalog 或目录指针。
- `ProviderService::update()` 先把共享供应商写入 DB，随后仅判断旧单 Home 语义下的“effective current provider”是否为该供应商；当前读到的部分没有遍历 Codex Profiles，也没有调用 Profile route manager。
- 这意味着编辑某个正被多个 Profile 引用的共享供应商时，数据库定义会立即变化，但各 Profile 的运行时快照和磁盘目录可能继续持有旧 Provider/旧 JSON，直到显式切换或重启 CC Switch 的其他恢复动作；共享源与派生状态存在系统性失配窗口。
- `ProviderService::update()` 尾部仅处理旧全局 live / takeover：如果编辑的是旧语义下的 current provider，就刷新默认 live 或全局 proxy backup；否则只保存 DB。它没有 Profile route manager 的引用，也没有任何 Profile 广播。
- 全仓针对 `list_codex_provider_profile_refs()` 的生产用法只有两处：提供 UI 查询、删除前防止引用悬空；不存在供应商更新后的 Profile 同步通知。
- 因此没有隐藏机制会在供应商保存后刷新 `~/.codex-api`：当前普通文件之所以存在，只可能来自此前关闭态直连切换、手工处理或其他单点写入，而非路由开启/共享变更传播。
- Git 归因显示显式 Home 直连目录写入及“其他 Home 不得生成目录”的测试均在同一个提交 `cfd833c3`（2026-07-15）引入；此前并不存在多 Home 同步行为被意外删除。
- 该提交解决的是“路由关闭态切换时把直连配置正确写到所选 Profile Home”，其事务边界有意限定为单 Profile；它没有覆盖“路由开启时首次生成目录”或“共享供应商编辑后传播到引用它的 Profiles”。
- 因此这是多 Profile 功能扩展后的需求覆盖缺口，而不是最近一次指纹/关闭路由修复造成的回归。
- 验证尝试 1 无效：使用 `cargo test ... -- --exact` 但未提供完整模块路径，输出 `0 passed / 2111 filtered out`，不能证明行为。后续改用唯一测试函数名的子串过滤，不重复该命令。
- 验证尝试 2 有效：改用测试名子串后实际运行 `codex_profile::home_config::codex_home_config::direct_provider_model_catalog_is_scoped_to_profile_home`，结果 `1 passed`。现实现稳定保证“关闭态直连只写目标 Home，不写其他 Home”。
- 提交 `cfd833c3` 的原设计文档明确将 `enabled=true` 定义为仅执行 runtime snapshot 热切换，将 catalog 写入限定在 `enabled=false` 的显式 Home 直连 plan；测试策略同样只要求“目录只写当前 Profile Home”。代码与设计完全一致，因此问题不是实现偏离设计，而是设计没有覆盖新的共享传播需求。
- 调查文档已通过 `git diff --check`；当前仅新增本 findings，未修改任何实现文件。

## 假设与验证

- 单一根因假设已通过 Git 历史验证。下一步执行现有的单 Home 作用域测试，并基于同一构建器做一个不改实现的最小行为核对，确认“直连会写、路由开启不会写”的模式稳定可复现。

## 根本原因

- 多 Profile 架构已经把供应商定义放在共享数据库中，并让每个 Profile 只保存供应商引用；但 `cc-switch-model-catalog.json` 仍被当成“切换直连供应商时顺手写入当前 Home 的辅助文件”，没有升级成由共享供应商变更驱动的派生投影。
- 具体断点有三个：
  1. 开启路由/启动恢复只改路由 TOML 和 runtime snapshot，不为当前 Home 生成目录；
  2. 路由开启态切换供应商只换 runtime snapshot 和 DB 引用，不刷新当前 Home 目录；
  3. 编辑共享供应商只更新旧单 Home live/全局 proxy 语义，不枚举引用该供应商的 Profiles，也不刷新它们的目录或 runtime snapshot。
- `model_catalog_json` 使用相对文件名，Codex 会从每个 `CODEX_HOME` 解析同名文件；因此只在默认 `~/.codex` 有文件时，`~/.codex-api` 不可能自动读取它。手工软链接只是绕过了缺失投影，且后续原子写会把符号链接替换成普通文件，不能作为稳定机制。
- 不能把“所有 Home 永远共用同一份目录内容”直接当作修复目标，因为不同 Profile 可以同时引用不同供应商；正确传播范围必须由每个 Profile 的 `current_provider_id` 决定：同一供应商的编辑应影响所有引用它的 Home，不同供应商的 Home 必须保持各自目录。

## Brainstorming 决策

| 决策 | 理由 |
|------|------|
| 供应商模型映射的修改传播到所有引用该供应商的 Profiles | 供应商定义是全局共享的，Profile 只保存引用；所有引用者应在下次启动 Codex 时看到相同的新模型映射 |
| Profile 切换供应商只影响该 Profile | 每个 `CODEX_HOME` 的供应商选择与路由策略相互隔离，不能因一个 Profile 切换而改变其他 Profile |
| 不采用“所有 Home 无条件指向同一个目录文件” | 不同 Profiles 可以同时引用不同供应商，单一全局目录无法表达这种状态 |
| 供应商不需要模型目录时只移除 CC Switch 管理的 TOML 指针，保留磁盘 JSON | 现有逻辑就是以 `model_catalog_json` 指针作为生效开关；无指针时残留 JSON 不会被 Codex 加载，保留文件也方便后续重新启用 |
| 已有正确指针的引用 Home 在模型映射更新时只重写 JSON | Codex 下次启动会读取新内容，无需无意义地改写 `config.toml`；仅首次使用、指针缺失/错误、或供应商在有目录与无目录之间切换时才需要更新 TOML |
| 多 Home 投影采用全有或全无的补偿语义 | 同一共享供应商不能在不同引用 Home 中留下不同版本；任一 Home 因权限、指纹冲突或写入失败时，恢复已修改 Home，并让供应商保存返回失败 |
| 采用“每 Home 独立投影副本”方案 | 保留 `model_catalog_json` 相对路径及跨平台兼容；允许不同 Profiles 同时选择不同供应商；以共享 DB 为唯一真相源，供应商修改时只扇出到引用它的 Homes |
| 不采用每供应商绝对路径共享文件 | 会重新引入此前为 WSL、符号链接和 Home 可迁移性而移除的绝对路径问题 |
| 不采用 Home 软链接 | 原子替换可能打断软链接，Windows 权限与补偿所有权也更复杂，不适合作为稳定生命周期机制 |
| 架构采用共享 DB + 每 Home 派生投影 + 跨 Home 协调器 | `Provider.settings_config.modelCatalog` 是唯一真相源；`CodexHomeConfigService` 只负责单 Home 原子计划；协调器只负责引用查询、锁排序、批量应用和反向补偿，符合单一职责 |
| 默认 `~/.codex` 不设特殊分支 | 默认 Home 与其他 Profiles 具备相同权限和行为，全部由 `current_provider_id` 决定投影内容 |
| Profile `/models` 保持空响应 | 当前设计有意避免读取默认 Home 造成跨实例泄漏；模型能力继续由各 Home 启动时加载的本地目录提供 |
| 路由开启、启用态切换和关闭态直连统一执行当前 Home 目录投影 | 消除“必须先关闭路由切一次供应商才生成目录”的隐含前置条件；路由状态只决定请求转发，不再决定模型目录是否生成 |
| 共享供应商编辑只扇出到以其为主供应商的 Profiles | Home 模型列表表达当前主供应商；仅作为 failover 的供应商不应改变当前可见模型目录 |
| CC Switch 启动时按 DB 权威状态自愈所有 Profile 投影 | 修复崩溃窗口、旧版本缺文件、手工删除和内容过期；单 Profile 失败记录错误但不阻止其他 Profiles 恢复 |
| 目录同步锁与排序后的 Profile 锁共同串行化写入 | 防止供应商编辑与路由启停/切换并发覆盖同一 Home，并通过固定锁顺序避免死锁 |

## 文档产出与自审

- 路径已按硬约束校验：
  - `docs/superpowers/findings/2026-07-16-codex-model-catalog-multi-home-findings.md`
  - `docs/superpowers/specs/2026-07-16-codex-model-catalog-multi-home-design.md`
  - `docs/superpowers/plans/2026-07-16-codex-model-catalog-multi-home.md`
- 首轮占位符扫描未发现任何禁用占位词、延后实现或模糊测试描述；`git diff --check` 未发现空白错误。
- 联合自审发现计划需要明确“供应商必须先无副作用校验/规范化，再写 Home”。计划将增加 Codex 更新预检方法，并让目录投影与最终提交使用同一份预处理 Provider，避免 spec 与执行顺序不一致。
- 类型一致性扫描发现计划示例引用了尚未定义的 Provider helper 和批量/路由测试夹具。计划已补充具体 helper 定义，避免实施者需要猜测 fixture 结构。
- 已移除未定义的批量/路由测试夹具，改为使用临时 Home、注入式 `CodexHomeFileOps` 以及现有 route manager 测试协作者；Provider helper 已给出具体构造，token store 也已改为实际字段初始化。
- 代码片段编译可行性扫描确认 `AppError` 没有通用 `From<std::io::Error>`；计划测试片段中的直接文件读写需要使用现有测试风格的 `expect` 或显式 `AppError::io` 映射。
- 已定位计划中全部直接 `fs` 调用；注入式文件操作继续显式映射 `AppError::io`，测试准备与断言改用 `expect`，避免给实施者留下无法编译的 `?` 示例。
- 一致性复核发现两项需修正：主引用筛选示例不应通过 `filter_map(...ok())` 吞掉数据库错误；旧的单 Home 直连作用域测试仍是正确的底层契约，不应改写成跨 Home 批量断言，跨 Home 行为应由新增 batch 测试覆盖。
- 项目已有 `src-tauri/src/codex_profile/constants.rs` 统一保存 Profile 领域常量；启动自愈错误前缀应在该文件新增常量，并只清理由该前缀标记的目录错误，避免覆盖其他路由生命周期错误。
- 第二轮规范扫描结果：三份文档均无禁用占位词、尾随空白或未闭合代码围栏；总计 findings 137 行、spec 244 行、plan 900 行。计划较长来自逐步 TDD 命令、具体 Rust 签名和补偿测试代码，不含重复的替代方案讨论。
- 主引用筛选示例已改为显式循环并传播 `get_route` 错误；旧单 Home 测试保留，新增 batch 测试承担跨 Home 语义。
- 提交规范已核对 `generate-commit` 及其分析指南：本次仅新增三份 Markdown，提交类型使用 `docs`，作用域使用 `codex`，不包含破坏性变更标记。
- 暂存区联合审查发现批量执行器不能只返回成功数量，否则后续供应商提交失败时没有完整计划可供恢复；设计和计划已改为返回持有成功计划的 receipt，并提供显式整批恢复函数。
- 供应商事务与启动自愈的计划代码块已补成具体函数体；启动错误保存/清理失败只记录警告并继续其他 Profiles，符合单 Home 故障隔离要求。
- 最终静态扫描未发现禁用占位词、未定义测试 fixture、旧返回签名、直接 IO 错误转换、尾随空白或未闭合代码围栏；唯一匹配的 `fs::remove_file(...)?` 已显式映射为 `AppError::io`，可编译语义正确。
- 完成前验证尝试 1 在 Cargo 测试前提前退出。逐项诊断确认文件清单、禁用模式与尾随空白均正常；原因是 findings 没有代码围栏，`rg -c` 在零匹配时返回空输出，后续算术检查失败。下一次验证把零匹配显式归一化为 `0`，不重复原脚本。
- 完成前验证尝试 2 通过：暂存区严格为三份目标文档，`git diff --cached --check`、禁用模式、尾随空白和代码围栏检查均通过；现有单 Home 目录测试实际运行 `1 passed / 0 failed`。

## Brainstorming 补充发现

- 旧全局代理的 `GET /v1/models` 直接读取默认 Home 的 `config.toml` 与默认 Home 的生成目录；它不适用于 Profile Home。
- Profile 独立监听器注册了单独的 `profile_models` handler，因此需要继续核对该 handler 的实际返回值，不能把旧全局 `/models` 行为套到多 Profile 上。
- `profile_models()` 固定返回 `{"models": []}`，注释明确要求“不读取默认 CODEX_HOME，避免跨 Home 配置泄漏”。因此 Profile 模型列表的正确数据源就是每个 Home 自己的 `model_catalog_json` 文件，不需要为本次目录同步额外改造 runtime `/models` 或热更新供应商快照。
- 用户允许 Codex 下次重启后生效，恰好与本地模型目录的加载时机匹配；本修复可聚焦磁盘投影与供应商保存事务，不扩张到请求运行时。
- `AppState` 已同时持有共享 DB、旧全局 `ProxyService` 与 `Arc<CodexRouteManager>`；通用 `update_provider` 命令当前是同步命令，只调用 `ProviderService::update()`。
- Profile enable/switch 命令均为 async，并通过 `CodexRouteManager` 获取 Profile 锁。供应商编辑要与这些生命周期操作互斥，命令层需要支持异步协调，不能在未持有 Profile 锁时直接遍历并写多个 Home。
- 设计边界应保持清晰：`ProviderService` 继续负责供应商校验、规范化、DB 和旧默认 live 语义；新的目录投影协作者只负责“计算引用 Home → 准备/应用/补偿目录计划”，不接管供应商业务逻辑。
- `CodexProfileRoutePersistence` 已提供 `list_profiles()` 与 `get_route()`，协调器可直接筛选 `route.current_provider_id == provider.id`；无需复用 `list_codex_provider_profile_refs()`。
- 现有 `list_codex_provider_profile_refs()` 的 SQL 同时返回主供应商与 failover 引用，只适用于删除保护和 UI 提示，不能用于目录扇出，否则仅作为 failover 的供应商也会错误覆盖当前 Home 模型目录。
- `update_provider` 需要改为 async，复用 `AppState.codex_route_manager` 的异步锁协调；现有 Tauri async Profile 命令已经提供相同调用模式。
