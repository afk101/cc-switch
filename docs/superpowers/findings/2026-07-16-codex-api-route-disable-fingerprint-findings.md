# `~/.codex-api` 关闭路由时指纹冲突调查

## 用户提供的第一手信息

- 现象：`~/.codex-api` 对应的 `CODEX_HOME` 无法关闭路由。
- 触发动作：在 CC Switch 中关闭该 Profile 的路由。
- 错误：路由变更失败，补偿恢复也失败；提示 Codex Live 配置已被外部修改。
- 截图中的预期指纹：`0cd9f15cf1355ca21455a625cf1fb27f999c88189151f38417b2723822e0b025`
- 截图中的实际指纹：`da1f19717effd69ee2d038b6f518a7ea95a40bbe0171e3858700c2e01cdb9133`
- 预期行为：能够正常关闭该 Profile 的路由，并恢复关闭路由前的 Codex 配置。
- 影响特征：用户反馈“总是出现”，表明问题具有稳定复现性。

## 调查记录

- 代码中的错误来自 `CodexHomeConfigService::restore_decoded_backup`：关闭路由时，只有当前配置等于备份的 `previous_fingerprint`、等于 `target_fingerprint`，或被严格识别为同端口旧版托管占位配置时才允许恢复；否则拒绝覆盖。
- 因此截图能够确定：关闭时磁盘上的 `config.toml` 既不是接管前原配置，也不是数据库认为由本次路由接管生成的目标配置，而且没有通过旧占位所有权识别。
- 2026-07-15 的既有现场记录显示，同一 `.codex-api` 已发生两轮同类故障：配置被写成 `127.0.0.1:15722/v1` 并包含 provider 级 `experimental_bearer_token`，数据库补偿停在 `disable_home_restore_failed`。
- 第二轮曾确认一个旧版开发进程路径：点击供应商卡片会在路由关闭态隐式调用 `enable_codex_profile_route`，再次接管 `.codex-api`。
- 本次数据库现场：Profile ID 为 `b341359e-437d-452c-a7ee-1019d99ae939`，监听端口 15722，route 仍为 `enabled = true`。
- `live_backup_json.previous_fingerprint = cb9f661d...`，`target_fingerprint = 0cd9f15c...`；截图中的“预期指纹”与数据库 target 完全一致。
- `recovery_json.operation = disable`，`phase = disable_home_restore_failed`，目标状态为禁用；说明第一次失败后操作记录被有意保留，后续点击会先恢复这条未完成操作。
- 当前配置仍选择 `model_provider = "custom"`，活动 provider 仍指向 `http://127.0.0.1:15722/v1`，并仍含 `experimental_bearer_token`，即磁盘仍处于路由接管形态，没有成功恢复。
- 当前 `config.toml` 的文件修改时间为 2026-07-15 19:26:16；需要用项目的“存在标志 + 文件字节”算法计算，而普通 `shasum` 不能直接与错误指纹比较。
- 按项目算法在文件字节前加入存在标志 `0x01` 后，当前配置指纹为 `da1f19717effd69ee2d038b6f518a7ea95a40bbe0171e3858700c2e01cdb9133`，与截图“实际指纹”完全一致；排除 UI 缓存、选错 Home 或展示旧错误。
- 从 `live_backup_json.previous_content` 原始字节复算得到 `cb9f661d...8132e`，与数据库 `previous_fingerprint` 完全一致；备份本身未损坏。
- 正式安装的 CC Switch 3.17.0 自 2026-07-15 19:24:38 运行，当前确实监听 `127.0.0.1:15722`；配置在进程启动约 98 秒后于 19:26:16 被写入。
- 当前失败记录的 `updated_at` 为 2026-07-16 10:02:49，说明今天点击关闭只是更新/重放补偿状态，磁盘配置自昨晚以来未再变化。
- Profile 私有 `listener-token` 创建于 2026-07-14 17:21:15；其 SHA-256 与当前 `config.toml` 中 `experimental_bearer_token` 的 SHA-256 完全一致。排除 token 被轮换、重建或串用导致本次指纹分叉。
- 将配置中的 token 替换为私有 token 后指纹保持 `da1f1971...`，进一步证明差异来自 token 以外的配置字段。
- 已核对：目标指纹过期来自 Codex Desktop 对同一配置文件追加自身设置，不是 listener token 或 CC Switch Profile 写入串用。

## 当前假设

- 假设 A 已确认并细化：Codex Desktop 插件/能力同步写入与路由无关的段落，但 CC Switch 的整文件所有权指纹没有容纳这类合法合并，导致 `live_backup_json.target_fingerprint` 过期。
- 假设 B 已确认：数据库确实保留上次失败的 `disable_home_restore_failed` 补偿记录，每次点击关闭只是在重放同一个旧指纹恢复，因此表现为“总是出现”。这是重复出现的直接机制，但仍需确认第一次指纹分叉的源头。
- 当前正式进程、磁盘时间和单变量指纹复算共同确认：第一次分叉发生在正式版启动后 Codex Desktop 写入 `js_repl`、插件与 MCP 段落时；token 假设已排除。

## 已确认根因

- CC Switch 启用路由时生成的目标配置只到 `[features]` 下的 `goals = true` 为止，其完整指纹为数据库保存的 `0cd9f15c...b025`。
- Codex Desktop 随后合法地向同一份 `~/.codex-api/config.toml` 追加/写入了以下自身状态：
  - `js_repl = false`
  - `[desktop]` 跟进队列设置
  - `[marketplaces.openai-bundled]` 及 `last_updated`
  - Browser、Chrome、Computer Use、Visualize、Sites 插件启用状态
  - `node_repl` 与 `computer-use` MCP 配置和相关环境变量
- 当前文件的 marketplace `last_updated = "2026-07-15T11:26:16Z"` 与文件本地修改时间 `2026-07-15 19:26:16 +0800` 精确对应，证明这批内容是 Codex Desktop 在启动/插件同步时写入，而不是 token 串用。
- 从当前文件删去 `js_repl = false` 及其后 Codex Desktop 追加的所有段落后，按项目算法复算出的指纹**精确等于**数据库 target：`0cd9f15cf1355ca21455a625cf1fb27f999c88189151f38417b2723822e0b025`。根因由单变量复算确认。
- 设计缺陷是：`restore_profile_backup` 用整份 `config.toml` 的逐字节 SHA-256 判断所有权。Codex Desktop 对与路由无关字段的正常更新，也被等同于真正的外部冲突；因此关闭操作被安全保护拒绝。
- 第一次关闭失败后，route manager 保留 `operation = disable / phase = disable_home_restore_failed`。后续每次点击关闭都会先执行 `recover_pending_locked`，再次拿同一个旧 target 指纹校验同一个已扩展的文件，因此稳定重复，而不会自行恢复。

## 已排除方向

- 不是选择了错误的 `CODEX_HOME`：实际指纹由 `~/.codex-api/config.toml` 精确复算得到。
- 不是 UI 缓存或旧错误文案：数据库当前 target 与截图预期值一致，现场文件与截图实际值一致。
- 不是 `experimental_bearer_token` 被改坏：配置 token 与 Profile 私有 listener token 的哈希及内容比较均一致。
- 不是备份损坏：`previous_content` 原始字节复算值与 `previous_fingerprint` 一致。
- 不是今天点击关闭再次改写配置：文件修改时间停留在昨晚，今天仅补偿记录时间更新。

## 结论

- 直接原因：Codex Desktop 在路由启用后正常扩展了自己的 `config.toml`，令 CC Switch 保存的整文件目标指纹过期。
- 重复原因：未完成关闭状态会在每次 mutation 前强制重放，而重放仍使用已经过期的整文件 target 指纹。
- 错误文案中的“已被外部修改”在这里具有误导性；写入者是 Codex Desktop 自身，且修改的是与路由所有权无关的配置段。

## Brainstorming 研究补充

### 现有边界

- `CodexRouteBackup` 只保存接管前整文件正文、previous 整文件指纹和 target 整文件指纹，没有记录“CC Switch 实际拥有的字段”或字段级基线。
- `build_codex_profile_route_toml` 实际只拥有少量路由字段：活动 provider 的 `base_url`、`wire_api`、`experimental_bearer_token`，以及供应商明确要求时的顶层 `model`。
- `toml_edit::DocumentMut` 已在 `codex_config.rs` 中用于语法保留式字段读写，具备实现字段级比较与合并的现成基础。
- 当前安全测试只覆盖两端：整文件完全等于 target 时允许恢复；明显改成外部 provider/base URL/token 时拒绝。缺少“路由字段不变、Codex Desktop 修改无关段落”这一合法并发写入场景。
- `recover_pending_locked` 对 `disable_home_restore_failed` 会无条件再次调用同一恢复方法；只要所有权判断策略不变，补偿记录就会永久重放失败。

### 设计约束

- 关闭路由不能整文件覆盖回旧备份，否则会删除 Codex Desktop 在路由期间新增的插件、MCP、marketplace、桌面设置等有效配置。
- 关闭路由也不能只看端口就无条件放行；如果用户把活动 provider、路由 token、base URL 或路由期间的 model 改成外部值，必须拒绝覆盖。
- 恢复结果需要采用三方语义：以接管前配置为恢复意图，以接管目标为所有权基线，以当前配置为需要保留的最新状态。
- 数据库兼容应优先复用现有 `previous_content + target_fingerprint`，避免为了修复现场故障再引入 schema 迁移；目标配置可由 previous 内容、Profile 端口、当前供应商与私有 listener token 重新构造。

### 待确认的产品语义

- 已确认：路由期间如果 Codex Desktop 或用户修改了与路由无关的字段，关闭后必须保留这些最新修改，而不是退回接管前整文件。

### 原有全局路由关闭逻辑回溯

- 原有入口 `ProxyService::set_takeover_for_app(app_type, false)` 调用 `restore_live_config_for_app_with_fallback_inner`，然后删除 `proxy_live_backup` 并清除 enabled 状态。
- 当 Codex 备份存在且不是异常代理占位符时，旧逻辑会把 `backup.original_config` 解析为完整 Live settings，再通过 `write_live_config_for_app` / `write_codex_live` 写回；它没有像 Profile 路由一样先做整文件 target 指纹校验。
- 因此旧逻辑不会出现本次“Codex Desktop 改了无关字段便完全关不掉”的安全拒绝，但它的默认恢复语义仍是**完整备份覆盖**，不是可靠的三方合并：接管期间新增、且未被专门同步进 backup 的字段可能丢失。
- 旧逻辑只在特定路径做过局部保留：
  - 代理活动时同步/热切供应商，会调用 `preserve_codex_mcp_servers_from_existing_config`，把现有 `mcp_servers` 合并进将要保存的备份。
  - 同时有 `preserve_codex_auth_in_backup` 保护 OAuth/API key 语义。
  - 这些保护是针对热切换重建 backup 的补丁，不是关闭路由时对当前 `config.toml` 做通用字段合并；也没有覆盖 desktop、marketplaces、plugins、features 等 Codex Desktop 当前会写入的全部区域。
- 结论：原有路由模式“能关闭”主要因为它不做严格整文件指纹拒绝；它已有 MCP/auth 局部保留思想，但没有满足本次确认的“关闭时保留所有非路由最新字段”语义，不能原样复用整文件恢复。
- 进一步确认 `write_codex_live -> write_codex_live_verbatim`：旧关闭链路最终根据备份中的完整 `config` 文本原子写回 `config.toml`；除模型目录投影分支外，没有读取关闭瞬间的当前 TOML 并做通用合并。
- 原有 `preserve_codex_mcp_servers_from_existing_config` 只会在代理活动期间重建供应商备份时执行；如果 Codex Desktop 在最后一次 backup 更新之后再新增插件/MCP/desktop 配置，关闭仍可能用旧备份覆盖掉它们。

### 方案设计输入

- 可以复用旧实现的思想：对 TOML 做结构化、按所有权合并，而不是继续维护越来越长的“允许 Codex Desktop 修改的字段白名单”。
- 不能直接复用旧实现的关闭函数：它绑定默认 `CODEX_HOME`、全局 `proxy_live_backup` 和完整 settings 写回，不符合多 Profile 隔离边界。
- 新 Profile 路由需要把通用三方合并能力放在 `CodexHomeConfigService` / `codex_config` 的纯函数边界内，由 Profile manager 提供 Home、端口、provider 与 listener token；不得重新依赖默认 Home。

### 用户确认的所有权边界

- 关闭路由时仅检查 `base_url`、`wire_api`、`experimental_bearer_token` 三个字段。
- `model` 即使在路由期间变化也保留关闭瞬间的最新值，不参与冲突判断或接管前值恢复。
- plugins、MCP、marketplaces、desktop、features 及其他所有非路由字段同样保留当前值。
- 三个字段的作用：
  - `base_url`：Codex 请求发送到的服务地址；Profile 路由启用时改为该 Home 的独立本地监听地址，如 `http://127.0.0.1:15722/v1`。
  - `wire_api`：该 provider 与服务端之间使用的请求协议；路由入口按 OpenAI Responses API 接收，因此接管值固定为 `responses`。
  - `experimental_bearer_token`：Codex 生成 `Authorization: Bearer ...` 所用的 provider 级 token；在 Profile 路由中它是本机 listener token，用于让 15722 只接受属于该 Profile 的请求，代理校验后会在转发上游前移除，不能与上游供应商 API key 混用。

### 方案比较与选择

| 方案 | 权衡 | 结论 |
|------|------|------|
| 三个路由字段的三方合并 | 不改数据库结构；兼容既有 backup 和卡住的补偿记录；需要实现 TOML 字段级读取、校验和恢复 | 用户选择，推荐 |
| backup 增加字段级所有权清单 | 所有权描述更显式，但需要备份格式版本兼容，并增加不必要的持久化复杂度 | 不采用 |
| 整文件指纹忽略 Codex Desktop 白名单区域 | 改动较小，但新版本增加配置段后会再次误判，无法长期覆盖未知非路由字段 | 不采用 |

### 技术决策

| 决策 | 理由 |
|------|------|
| 关闭 Profile 路由使用字段级三方合并 | 同时满足保留当前非路由配置、恢复接管前路由值、拒绝覆盖真正路由冲突三项目标 |
| 严格所有权字段仅为 `base_url`、`wire_api`、`experimental_bearer_token` | 用户明确确认；三者共同决定请求目的地、协议和 Profile 本地认证 |
| `model` 与所有其他字段采用关闭时当前值 | 它们不属于路由传输身份，不能因关闭路由回滚 Codex Desktop 或用户的最新选择 |
| 不新增数据库 schema 或 backup 字段 | 现有 `previous_content` 足以提供接管前值，Profile 端口和私有 token 足以验证当前路由身份；避免迁移并让现有失败现场直接恢复 |
| TOML 合并保持在 Home 配置服务的纯逻辑边界 | route manager 只负责生命周期与凭证供应，配置服务单独负责字段所有权和可测试的文档变换，符合单一职责 |

### 已批准设计：架构边界

- 用户批准将字段级所有权与三方 TOML 合并封装在 `CodexHomeConfigService` 内，route manager 只提供 Home、Profile 端口和 listener token。
- 所有权位置由接管前配置推导：有活动 provider 时锚定对应 `[model_providers.<id>]`，否则锚定顶层；关闭期间 `model_provider` 的其他变化不扩大 CC Switch 的字段所有权。
- 三个严格字段匹配当前 Profile 路由身份后，仅在原接管位置恢复它们接管前的存在/取值状态；其他当前字段全部保留。
- 整文件指纹继续用于应用计划时的 CAS 并发保护与诊断，不再作为 Profile 关闭恢复的唯一门槛。

### 数据流与补偿研究

- 正常 disable 与 pending disable recovery 当前都会在恢复 Home 后再排空/停止 runtime；修复应让两条路径调用同一个字段级幂等恢复函数，避免正常路径修好而启动补偿仍卡住。
- `clear_operation` 只清理 recovery/error；现有 disable 成功后仍保留 `live_backup_json`。本次不改变该既有生命周期语义，避免扩大范围。
- 字段级恢复必须识别两种成功前置状态：
  - 三个字段仍等于路由目标：执行三方合并并原子写入。
  - 三个字段已经等于接管前状态：说明 Home 已在上次尝试中成功恢复，直接幂等返回并继续停止 runtime/提交 DB。
- 这能覆盖“Home 已写回后进程崩溃”或“Home 写回成功但停止 runtime 失败”的重试窗口；若只接受路由目标值，第二次补偿会把自己上次的成功恢复误判为外部修改。
- listener token 在关闭时只应用于证明当前配置归属，不应因为文件缺失而静默生成一个新 token；缺少原凭证时无法证明所有权，应保留补偿状态并返回脱敏错误。
- 字段验证完成到原子 rename 之前仍存在并发写窗口；写入前应再次确认当前整文件指纹等于本轮读取值。这里的整文件指纹只充当短事务 CAS，不限制路由期间的长期合法修改。

### 已批准设计：数据流与错误处理

- 用户批准正常关闭与 pending disable recovery 共用字段级幂等恢复流程。
- route manager 在锁内读取既有 listener token，不因关闭操作补建凭证；Home 收敛后再排空、停止 runtime 并提交禁用状态。
- 三态判断为：路由目标态执行合并；接管前字段态幂等继续；其他状态判定严格字段冲突。
- 原子写入前使用本轮读取的整文件指纹做短事务 CAS，避免校验与 rename 之间覆盖并发写入。
- 错误必须脱敏：base URL / wire API 可报告值，token 仅报告缺失或不匹配；解析、凭证、写入失败都保留补偿记录。

### 已批准设计：测试、兼容与验收

- 用户批准以 Home 配置服务单元测试、route manager 生命周期测试和完整 Codex Profile 后端测试覆盖实现。
- 必测场景包括：Codex Desktop 非路由段保留、model 保留、原字段存在/缺失恢复、三个严格字段逐项冲突、token 错误脱敏、幂等补偿、顶层/Provider 两种位置、并发 CAS、原子写失败和多 Profile 隔离。
- 不修改 `live_backup_json` 格式，不新增数据库迁移，不改变旧全局代理逻辑或前端。
- 自动测试使用临时 Home；新版本通过同一 pending disable recovery 流程收敛当前 `.codex-api` 现场状态，不在实施测试中直接修改用户现场配置。

### 实现计划代码定位

- `home_config.rs:71-77` 的既有 `CodexRouteBackup` 保持序列化形状不变；字段级位置和值从 `previous_content` 解析，不持久化新元数据。
- `home_config.rs:331-365` 的 `restore_profile_backup/restore_decoded_backup` 是替换整文件恢复门槛的主要位置；普通 `restore(plan)` 与模型目录恢复继续使用整文件 CAS，不在本次范围内。
- `route_manager.rs:70-83` 的 `CodexProfileTokenStore` 需要增加只读 token 方法，并由真实 `CodexProfileSecretStore::read` 实现；`ensure_token` 继续只用于启用/恢复需要创建凭证的路径。
- `route_manager.rs:591-655` 正常 disable 与 `route_manager.rs:1259-1299` pending disable recovery 两处调用必须统一传入只读 token，并复用一个 manager 私有恢复入口，避免分支漂移。
- 测试替身 `TrackingTokenStore` 与 `FailingDeleteTokenStore` 需要同步实现只读契约；新缺失 token 替身用于证明关闭不会调用 ensure/create。
- 新增字段名、固定 `responses` 值和脱敏错误标签应放入 `codex_profile/constants.rs`，符合项目常量集中约束。

### 写计划前的设计细化

- 三个字段不能共享一个简单的“顶层或 provider”位置：`update_codex_toml_field` 会把 `base_url/wire_api` 写进活动 provider 表，但 `set_codex_experimental_bearer_token` 对 Codex 保留 provider ID 会把 token 写在顶层；自定义 provider 才优先写入 provider 表。
- 因此字段位置必须逐字段推导。推荐从接管前正文调用现有纯构造器生成不含 model 变化的目标正文，再分别定位目标中三个接管值所在路径；这样复用现有 provider ID 规则，不复制一套容易漂移的判断。
- 三方恢复按三个独立路径读取 previous/target/current：target 只用于身份校验，previous 提供恢复值或缺失状态，current 作为保留非路由字段的输出基底。

### 文档自检记录

- finding、spec、plan 已完成占位词扫描，未发现 `TBD`、`TODO`、`implement later` 或模糊的“类似任务”表述。
- 下一步交叉检查设计与任务覆盖，重点核对旧 `restore_profile_backup` 调用点、字段投影返回类型和短事务并发校验的措辞，避免计划实现时出现签名或语义歧义。
- 第一轮交叉检查确认 spec 的职责、三方状态、补偿与脱敏要求一致；计划中 `build_managed_route_projection` 同时返回 `previous_doc` 又允许实现时临时调整，属于不必要的签名歧义，应在最终计划中确定唯一返回类型。
- 短事务二次读取是 best-effort 并发检测，不是跨进程原子 CAS；文档应保持“重校验后再原子替换”的准确表述，避免过度承诺。
- 调用点核对发现除正常 disable 与 pending recovery 外，`home_config.rs` 现有单测还直接调用旧三参数 `restore_profile_backup`；实施计划必须显式把该调用更新为新增 listener token 参数，不能只依赖编译报错临时发现。
- token store 真实实现和两类既有测试替身共三个实现点，计划已覆盖；正常关闭和 pending recovery 两个生产调用点也已安排收敛到同一 manager 私有入口。
- 现有 `is_legacy_managed_home` 只校验同端口 base URL 与旧 token，尚未校验 `wire_api`；计划必须把“旧占位兼容需同时匹配 responses”落实为测试和实现。
- `Option<String>` 无法区分“字段缺失”和“字段存在但类型不是字符串”，也无法原样恢复接管前的非字符串 TOML Item；字段状态应保存 `toml_edit::Item` 的缺失/存在形态，错误格式化再按字段做脱敏。
- current 文件缺失不能一概报错或一概视为已恢复：只有 previous 文件原本也不存在时，缺失 current 才是合法幂等结果；previous 文件存在时 current 缺失属于外部变更，应拒绝收敛。
- 修订后占位词复扫只命中 token store 合法的 `Option<String>` 与 findings 中对旧问题的说明，字段状态本身已改为保留原始 Item。
- projection 还需记录路由构造阶段新建的 provider 子表和 `model_providers` 父表；否则应用 previous 后无法可靠区分“本来就存在的空表”和“路由临时创建的空表”，会使清理规则缺少实现数据。
- 最终结构检查确认 spec 含目标、非目标、架构、三方算法、生命周期、错误安全、测试和验收章节；plan 含五个可执行任务，且关键字段、三态、只读 token、pending recovery、旧占位兼容和脱敏要求均有对应步骤。
- 当前工作区范围检查只发现本次新增的 finding、spec、plan 三份文档，没有混入实现文件或其他未提交修改。
