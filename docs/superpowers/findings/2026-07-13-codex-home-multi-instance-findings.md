# 发现与决策

## 需求
- 调研 CC Switch 是否只能基于默认 `CODEX_HOME` 管理 Codex 路由。
- 目标形态：默认 `CODEX_HOME` 保持官方订阅；一个或多个独立 `CODEX_HOME` 使用不同的路由配置，并且可以同时启动。
- 用户明确负责依据不同 `CODEX_HOME` 启动多个 Codex Desktop/App；CC Switch 不负责启动、重启或管理 Codex 进程，只维护多个配置、配置与 Home 的绑定以及路由识别。
- 用户确认供应商配置全局共享；每个 `CODEX_HOME` 仅保存供应商引用和路由策略。启用供应商时需要明确选择目标 `CODEX_HOME`。

## 研究发现
- `src-tauri/src/codex_config.rs` 的 `get_codex_config_dir()` 只会返回一个目录：设置中的单一 `codex_override_dir`，否则为用户主目录下的 `~/.codex`。随后的 `auth.json`、`config.toml` 和模型目录均由该函数派生。
- 同文件的 `get_codex_provider_paths()` 虽会保存多份 `auth-<provider>.json` 与 `config-<provider>.toml`，但它们都在同一个配置目录中，并非多个独立 `CODEX_HOME`。
- `src-tauri/src/proxy/provider_router.rs` 的供应商选择入口只接收 `app_type`；熔断器 key 为 `app_type:provider_id`。当前模型无法按 Codex 实例或 Home 选择不同活跃供应商。
- `src-tauri/src/settings.rs` 的 `get_codex_override_dir()` 读取全局设置字段 `codex_config_dir`；这是“一次只选一个目录”的应用级 override，不是可同时管理的实例列表。技能目录与 WSL 工具定位也复用这个单一 override。
- `src-tauri/src/codex_history_migration.rs` 的注释明确表明 Codex 会从 `CODEX_HOME` 解析状态目录，且 `config.toml` 可用 `sqlite_home` 覆盖状态数据库位置；这说明多 Home 场景还要处理会话历史/状态库隔离，而不仅是 `auth.json` 和 `config.toml`。
- 官方 Codex 文档的搜索结果明确说明：`CODEX_HOME` 可用于不同 profile，例如以 `CODEX_HOME=$(pwd)/.codex codex exec ...` 启动；环境变量用于 shell 范围覆盖，而持久设置使用 `config.toml`。
- OpenAI Codex 源码/文档搜索结果说明：未设置时 Home 默认为 `~/.codex`；设置 `CODEX_HOME` 后会作为“包含所有 Codex 状态”的目录。该目录还承载认证缓存和会话相关状态。故“默认 Home 订阅 + 多个路由 Home”在 Codex 进程层完全可行，只要每个进程启动时注入不同环境变量。
- OpenAI 文档还提示 SQLite 状态数据库可被 `sqlite_home` 或 `CODEX_SQLITE_HOME` 覆盖；设计应默认将每个实例完整隔离，避免只隔离认证/配置而遗漏状态库。
- `src-tauri/src/services/proxy.rs` 的 Codex 接管会读取当前 live 配置、以 `AppType::Codex` 构造有效供应商设置，再把接管后的配置写回 live 配置；调用链没有实例参数。
- `src-tauri/src/settings.rs` 的当前供应商字段固定为单个 `current_provider_codex`，不是映射表。多实例并存时，路由请求无法依据来源自动选择各自供应商。
- 会话管理、用量同步、历史迁移、MCP 同步、模型缓存和配置命令都调用 `get_codex_config_dir()`。正式设计必须让这些读写以实例 ID 解析 Home，而非只改切换供应商逻辑。
- 桌面端与 CLI 的启动边界不同：CLI 可在每次命令前直接设置 `CODEX_HOME`；用户从 Finder、Dock 或既有桌面进程打开 Codex 时，通常不会继承某个终端命令的环境变量。因此桌面端多实例需要由 CC Switch 通过受控启动器在进程创建前注入实例对应的 `CODEX_HOME`，并处理已运行实例。
- 上一条桌面启动器方向已被用户明确排除，不进入本功能范围。
- 当前 Codex 接管配置使用全局固定 `PROXY_TOKEN_PLACEHOLDER`。多个 Home 即使各自拥有独立 `config.toml`，请求进入同一本地路由端口后仍没有实例身份；需要为配置绑定生成稳定、不可猜测的本地路由凭证，或采用端口/URL 路径等其他实例区分机制。
- 数据库 `providers` 表以 `(id, app_type)` 为主键，并以单个 `is_current` 表达应用当前供应商；`proxy_config` 同样以 `app_type` 为主键。现有结构适合继续作为共享供应商目录，但不适合直接承载每个 Home 的当前选择。
- 前端 `ProviderList` 当前只接收一个 `currentProviderId`。多 Home 设计应先选择“Codex 实例上下文”，再复用供应商列表展示该实例的当前供应商，避免把实例和供应商混在同一层级。
- 代理服务器目前以固定 `/v1/responses`、`/v1/responses/compact` 等端点接收 Codex 请求，`RequestContext::new()` 随后仅以 `app_type="codex"` 选择供应商。
- Codex 接管会把 `experimental_bearer_token` 写入 live `config.toml`；请求处理链在选择供应商前已经拥有完整 HeaderMap。因此可以用每个 Home 的 Bearer token 解析实例 ID，无需改 Codex 请求体或增加监听端口。
- “内容不串”不能只靠 token 改供应商选择：`ProxyState.codex_chat_history` 当前是全局单例，内部仅以 `response_id`、`call_id` 索引跨请求工具调用历史。多实例后必须按 `profile_id` 分区，否则极端 ID 冲突或 fallback 查找可能恢复另一个 Home 的历史内容。
- `ProxyState.current_providers` 当前以 `app_type` 为 key，熔断器以 `app_type:provider_id` 为 key，故障转移管理也基于应用级当前供应商。它们虽不直接拼接正文，但会造成实例间路由策略和健康状态互相影响，均需引入 `profile_id`。
- HTTP 请求体、转换状态和 SSE 响应缓冲本身是请求局部对象；全局 HTTP Client 只复用连接池，不共享正文。真正需要隔离的是跨请求状态容器和数据库选择键。
- 仓库中文指南明确描述：路由接管会将 Codex 的 live `config.toml` 改为本机路由地址，路由按 CC Switch 中“当前 Codex 供应商”转发。这佐证它是单一 live 配置、单一当前供应商的模型。
- 现有“保留官方登录”能力将官方订阅登录态保留在同一个 `auth.json`，把第三方配置写入同一个 `config.toml`；它实现的是单个 Home 内“官方身份 + 第三方流量”，并不能让默认 Home 同时稳定保持官方直连、另一个 Home 同时走路由。

## 技术决策
| 决策 | 理由 |
|------|------|
| 先仅做代码与资料调研，不修改产品代码 | 用户当前明确要求先调研；头脑风暴流程要求设计获批前不得实现。 |
| 将问题拆为“Codex 进程配置隔离”和“CC Switch 路由供应商选择隔离”两层 | 仅设置不同 `CODEX_HOME` 只能隔离 Codex 的文件；若多个实例共享一个路由，仍会竞争 CC Switch 的单一当前 Codex 供应商。 |
| 将“多个可同时启动”的首选实现定义为一实例一监听端口 | 每个实例的 `config.toml` 可写入唯一的本地路由 URL，代理可根据监听器静态绑定实例，而不依赖请求体里脆弱、可被用户改写的识别字段。该方案后续已获用户确认。 |
| 曾将“一实例一监听端口”降为候选方案，继续比较单端口实例凭证方案 | 这是讨论中间状态；在用户强调“内容不能串”并明确选择多个独立路由后，此方向已被后续决策取代。保留记录用于说明方案演进，不作为实施依据。 |
| 推荐把供应商定义与 Codex 实例绑定分离 | 供应商配置可跨 Home 复用；实例只保存 Home、当前供应商和实例级策略，符合单一职责且避免重复密钥与模型配置。 |
| 采用共享供应商库 + Home 绑定模型 | 已获用户确认；同一 Home 可使用多个供应商，同一供应商也可被多个 Home 引用。 |
| Codex 页面采用“先选 Home，再操作供应商”的上下文模式 | 已获用户确认；避免每次启用供应商都弹出 Home 选择器，同时允许供应商卡片展示已绑定 Home，并提供批量应用入口。 |
| Home 生命周期：内置 `~/.codex`，自定义路径唯一，删除绑定不删除目录 | 已获用户确认；默认实例路径不可改删，自定义实例可重命名/重新绑定，避免误删 Codex 认证和会话数据。 |
| 默认 `~/.codex` 初始官方直连，但不永久锁死 | 已获用户确认；满足默认订阅场景，同时允许用户显式切换并在操作前给出风险提示。 |
| 曾推荐单端口 + Home 专属本地路由凭证 | 这是候选方案比较结论，后续因用户选择“一 Home 一独立端口”而否决。凭证仍保留为本地访问控制，不再承担实例分流职责。 |
| 监听器创建时构造不可变 `CodexProfileScope` 并贯穿请求全链路 | 仅在入口选一次供应商不足以阻止跨请求缓存、故障转移和状态映射串实例；独立端口负责选择 Profile，请求作用域负责约束后续组件。 |

### 路由实例识别候选

1. **Home 专属 Bearer token（候选，已否决）**：每个实例生成随机本地凭证，代理先解析凭证再按实例选供应商。单端口、扩展成本低，凭证同时承担本地访问控制；需要处理 token 轮换、未知 token 和迁移。
2. **URL 路径命名空间**：为每个实例写入 `/codex-profiles/<id>/v1`。可读性强，但要扩展所有 Responses/Compact/Models 路由，实例 ID 不是访问凭证，兼容面更大。
3. **每个 Home 独立端口（最终采用）**：实例与端口一一绑定。隔离最直观，但会引入端口分配、冲突、多个监听器和运行时配置复杂度。

### 用户提出：多个独立路由

- 用户追问能否为多个 Home 分别运行独立路由。若定义为“一 Home 一监听端口、一独立运行时状态”，技术上可行。
- 该方案仍在同一个 CC Switch 进程内运行，无需启动多个 CC Switch；`CodexRouteManager` 按 `profile_id` 管理多个 listener/runtime。
- 每个 runtime 必须独享 `CodexChatHistoryStore`、当前供应商快照、故障转移/熔断状态、状态统计和日志命名空间；仅供应商定义与底层无状态 HTTP 连接池可共享。
- 相比单端口 token 分流，该方案增加端口分配和监听器生命周期管理，但“内容不串”可由结构隔离直接保证，适合实例数量较少且强调隔离的桌面场景。

### 已确认的路由方案

- 用户选择“每个 Home 独立端口”，示例：工作实例使用 `15722`。
- 每个端口绑定且仅绑定一个 `profile_id`；handler 无需从请求内容推断实例，listener 创建时已经固定实例作用域。
- 本地路由凭证仍建议每个实例独立生成，用于拒绝误连，但它不再承担实例分流职责。
- 端口采用自动分配、允许手动修改、分配后持久化：现有 Codex 路由保留 `15721`；新 Home 从 `15722` 向上找可用端口。
- 保存时校验端口未被其他 Home 或系统进程占用；冲突只阻止该实例启动，不影响其他路由，也不允许静默换端口导致 `config.toml` 失配。

### 已批准的核心架构

- `CodexProfileRepository` 管理实例身份、名称与唯一 Home 路径。
- `CodexHomeConfigService` 的所有配置读写显式接收 Home 路径，淘汰 Codex 多实例链路对全局 `get_codex_config_dir()` 的隐式依赖。
- `CodexRouteManager` 管理 `profile_id -> RouteRuntime`；每个 runtime 绑定一个端口和一个 profile。
- 每个 runtime 独享 ProviderRouter 状态、Codex Chat 历史、状态统计和日志命名空间；仅共享供应商定义及无状态 HTTP 连接池。
- 持久化拆为 `codex_profiles`、`codex_profile_routes`、`codex_profile_failovers`，现有 `providers` 继续全局共享。
- Home 选择是 Codex 页面最高层上下文；供应商、路由、故障转移、会话、用量、MCP、Skill、配置备份与日志等 Home 相关能力都必须在该上下文内查询和修改。
- Profile 绑定只引用全局供应商记录；同一供应商可被多个 Profile 使用。不同 API Key 或不同秘密配置必须建成不同供应商记录，避免在 Profile 绑定层覆盖全局密钥。

### 已批准的操作数据流

- `CODEX_HOME` 是 Codex 页面的最高级上下文；用户先选 Home，后续所有实例相关操作均显式携带 `profile_id`。
- 启用流程按 profile 加锁，先校验并启动独立 runtime，再原子备份/写入目标 Home，最后提交数据库状态；失败反向回滚。
- 切换供应商保持端口与本地凭证不变；在途请求使用启动时快照，新请求使用新供应商。
- 关闭路由先恢复目标 Home 配置，再停止接收新请求并排空在途请求；每个 Home 的 `auth.json` 始终独立保留。
- 用户明确认可“先选定 CODEX_HOME，之后全部隔离”的总原则。
- Profile 删除前先恢复其 live 配置并排空/停止路由；任一步失败都保留 Profile 记录，绝不删除 Home 目录或目录内文件。
- 删除全局供应商时先检查 Profile 引用；存在引用则阻止删除并列出使用它的 Home。

### 已批准的错误与隔离边界

- Home 路径为空、无法规范化、重复、不可写或不是可用目录时，在任何数据库和配置副作用前拒绝操作。
- 端口被另一个 Profile 或系统进程占用时，只让目标 Profile 进入错误态；不得影响其他 Profile，也不得静默更换端口。
- 一个 runtime 崩溃、熔断、故障转移或供应商切换时，其他 Profile 的 runtime、请求历史和路由策略保持不变。
- 激活与关闭采用可回滚步骤；配置写入使用同目录临时文件加原子替换，失败后恢复 live 配置、runtime 和数据库状态。
- 写入前比较已记录的配置指纹；发现用户或 Codex 在 CC Switch 外部修改了 live 配置时，停止覆盖并提示冲突处理。
- 并发隔离测试必须使用相同的 `session_id`、`response_id` 和 `call_id` 向两个端口发送不同正文，证明聊天历史、工具调用和上游请求不会交叉。
- Profile A 在流式请求中切换供应商时，在途请求继续使用其启动快照；Profile B 不受影响。
- 每个 Home 的本地路由凭证只用于对应监听器的本地鉴权，必须脱敏保存和记录，且不得转发到上游。
- `CC_SWITCH_DUMP_BODY=1` 时，body dump 路径和文件名必须带 Profile 命名空间，便于审计且避免不同 Home 的请求日志混放。

### 实例权限修正

- 所有 Codex Profile 权限和能力完全同级；“默认 Codex”仅表示预置路径为 `~/.codex` 且该记录不可删除，不是特权实例。
- 任意自定义 `CODEX_HOME` 都可拥有自己的官方订阅登录态并使用 OpenAI Official，也可选择第三方直连或独立本地路由。
- UI 与后端不得用 `is_default` 限制官方供应商、订阅、路由、故障转移、会话或配置能力。
- 每个 Home 的官方登录态来自该目录自己的 `auth.json`；不得从默认 Home 复制或共享认证内容。

### 已批准的旧版本迁移

- 迁移不移动、不复制、不改写任何 Codex 文件，只新增 CC Switch Profile 关系。
- 旧目录为 `~/.codex` 时映射为默认 Profile；旧 override 为其他路径时，额外创建“原 Codex 配置”并保持为选中实例。
- 旧当前供应商、路由状态和 `15721` 归属旧实际目录对应的 Profile；已接管状态在升级后继续运行。
- 迁移幂等；旧单例字段迁移后仅用于兼容读取，不再承载新写入。

### 实现落点补充

- 当前数据库 schema 版本为 v11；实现计划需要新增 v12 migration，为 Profile、Route、Failover 补充独立 DAO，并增加 Profile-MCP、Profile-Skill 关联以及会话/用量归属字段。
- 前端 Codex 供应商主入口位于 `src/App.tsx`，统一列表为 `src/components/providers/ProviderList.tsx`；适合在 Codex 分支上方增加 Home 上下文组件，不复制供应商列表。
- 供应商 API 目前按 `appId` 获取单一 `currentProviderId`；Codex 分支需增加 profile-aware API/query key，非 Codex 应用保持原接口行为。
- `ProxyServer::new()` 已在实例内部创建独立 `ProviderRouter`、`CodexChatHistoryStore` 与状态容器；实现可复用其生命周期模式，但应提供 Codex Profile 专用 runtime，避免复制 Claude/Gemini 的全局接管逻辑。
- `ProviderService::switch` 与 `switch_proxy_provider` 目前只接收 `AppType/provider_id`；Codex 页面需要新增显式 `profile_id` 命令，不应改变其他应用的单例接口。
- 项目验证命令为 `pnpm test:unit`、`pnpm typecheck`、`pnpm build:renderer`；Rust 定向测试可用 `cargo test --manifest-path src-tauri/Cargo.toml <test-filter>`。
- `ProxyService` 当前只持有一个 `Option<ProxyServer>` 与 app 级 switch lock；计划应新增 Codex Profile 专用 manager，而不是让现有通用代理的 Claude/Gemini 状态被多个 Codex runtime 覆盖。
- 前端 `useProvidersQuery` 的 key 固定为 `["providers", appId]` 且启用 `keepPreviousData`；Codex Profile 查询必须包含 `profileId`，并避免在切换 Profile 时展示上一实例的 current/status 数据。
- Rust 侧当前没有统一 `constants.rs`；按项目规则在新的 Codex Profile 模块内创建专用 `constants.rs`。前端固定端口起点等常量放入现有 `src/config/constants.ts`。
- `proxy_live_backup` 当前以 `app_type` 为主键；Codex 多实例备份必须迁移为 profile 维度，不能让两个 Home 覆盖同一份回滚材料。
- 设置页当前仍提供单一 `codex_config_dir`；升级后该入口应替换为 Codex Profile 管理入口，旧字段仅承担幂等迁移来源。
- MCP、Skill、会话和用量相关 Codex 路径均存在默认目录隐式解析；既然 Home 是最高上下文，这些命令、查询和同步关系也要携带 `profile_id`，必要时增加 Profile-MCP/Profile-Skill 关联表及用量归属字段。

## 遇到的问题
| 问题 | 解决方案 |
|------|---------|
| 初次广泛文本搜索结果混入多语言资源，无法直接判断 `CODEX_HOME` 实现 | 改为聚焦 Rust 配置模块、路由接管服务、Codex 指南与测试。 |
| Exa 检索结果的 `jq` 展示表达式转义错误 | 不重复原命令，下一次直接输出 `{title,url,highlights}` 结构化 JSON。 |
| 官方手册 helper 未输出可用的本地手册路径 | 依照 OpenAI 文档技能的后备流程，改用官方域名的实时检索结果，并同时以 OpenAI Codex 源码结果交叉核验。 |
| 实现落点检索误写不存在的 `src-tauri/src/database.rs` | 改用实际入口 `src-tauri/src/database/mod.rs` 与 `src-tauri/src/database/schema.rs`，不重复错误路径。 |
| 首次文档占位符扫描把计划中的 TypeScript JSDoc `/** ... */` 误判为占位注释 | 改用只匹配非 JSDoc 块注释的 PCRE2 规则，同时单独校验计划要求的 JSDoc 示例仍存在，不重复原扫描。 |
| 安全存储检索错误地包含了仓库根不存在的 `Cargo.toml` | 停止复用该路径，改以实际 `src-tauri/Cargo.toml` 和已命中的 settings/sync 代码判断；本地路由 token 不进入会同步的数据库表。 |
| 计划结构检查发现类型导出入口误写为不存在的 `src/types/index.ts` | 检查 `@/types` 实际解析后改为现有入口 `src/types.ts`，保留新建的聚合类型模块 `src/types/codexProfile.ts`。 |
| zsh 结构检查脚本使用变量名 `path`，覆盖了 zsh 的特殊 `PATH` 数组，导致后续 `grep` 不可用 | 改用普通变量名 `file_path`，并以 `rg` 计数，不重复会污染 `PATH` 的脚本。 |

## 资源
- `src-tauri/src/codex_config.rs`
- `src-tauri/src/proxy/provider_router.rs`
- `src/config/codexProviderPresets.ts`
- `docs/guides/codex-official-auth-preservation-guide-zh.md`
- `docs/guides/codex-deepseek-routing-guide-zh.md`
- https://developers.openai.com/codex/environment-variables
- https://developers.openai.com/codex/guides/agents-md
- https://github.com/openai/codex/blob/d807d44a/docs/config.md

## 视觉 / 浏览器发现
<!-- 每执行 2 次查看/浏览器操作后必须更新此部分 -->
<!-- 多模态内容必须立即以文本形式记录 -->
- 本轮未查看 UI 截图或浏览器界面。

---
*每执行 2 次查看/浏览器/搜索操作后更新此文件*
