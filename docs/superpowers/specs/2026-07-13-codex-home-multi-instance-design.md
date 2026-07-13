# Codex 多 CODEX_HOME 实例设计

## 状态

- 日期：2026-07-13
- 状态：设计已逐段确认，待用户审阅本文与实施计划
- 范围：CC Switch 内的 Codex Profile 管理、多独立路由与 Profile 级功能隔离

## 背景

当前 CC Switch 可以保存多个 Codex 供应商配置，也可以接管 Codex 请求，但所有配置读写都从一个全局 `CODEX_HOME` 推导，当前供应商、路由开关、故障转移和跨请求聊天历史也都是 Codex 应用级单例。因此它能维护“同一个 Home 下的多个供应商”，不能同时维护并运行“多个 Home 各自独立的供应商选择与路由”。

目标不是把默认 `~/.codex` 固定成官方订阅、把其他目录固定成路由，而是把每一个 `CODEX_HOME` 都建模为能力完全同级的 Codex Profile。用户先选择 Profile，之后所有与 Home 有关的功能都在该作用域内操作。任意 Profile 都可以保留自己的官方订阅登录态、选择 OpenAI Official、使用第三方直连，或启动自己的本地路由。

用户负责以不同 `CODEX_HOME` 启动多个 Codex Desktop/App 实例。CC Switch 不负责启动、终止或重启 Codex 进程。

## 目标

- 在 CC Switch 中登记并清晰管理多个 `CODEX_HOME`。
- 将 Home 作为 Codex 页面的最高级上下文，后续操作都显式绑定 `profile_id`。
- 供应商定义全局共享，同一供应商可被多个 Profile 引用。
- 每个 Profile 可以同时运行独立端口、独立状态的 Codex 路由。
- 确保不同 Profile 的请求正文、工具调用历史、供应商健康状态和配置文件不会串用。
- 保留现有单 Home 用户的行为，并以幂等迁移升级到 Profile 模型。
- 不复制、不共享、不删除任何 Home 内的认证与会话数据。

## 非目标

- 不负责给 Codex Desktop/App 注入 `CODEX_HOME`，也不管理其进程生命周期。
- 不通过复制 `auth.json` 帮用户迁移或共享官方订阅。
- 不为每个 Profile 复制一套全局供应商密钥。
- 不提供操作系统进程级隔离；多个路由运行时仍位于同一个 CC Switch 进程。
- 不要求“默认 Profile 必须直连”或“自定义 Profile 必须路由”。
- 不在本期重新设计 Claude、Gemini 或其他应用的单实例路由模型。

## 术语

| 术语 | 定义 |
|------|------|
| Codex Profile | CC Switch 中一个已登记 `CODEX_HOME` 的稳定身份，简称 Profile。 |
| Home | Profile 指向的 `CODEX_HOME` 目录。 |
| 默认 Profile | 内置并指向 `~/.codex` 的 Profile；只有路径不可改、记录不可删这一生命周期差异。 |
| 供应商 | 现有全局 provider 记录，包含 API Key、Base URL、模型配置等。 |
| RouteRuntime | 一个 Profile 专属的本地监听器与全部有状态路由组件。 |
| live 配置 | 目标 Home 中 Codex 当前实际读取的 `config.toml`、模型目录等文件。 |

## 核心不变量

1. 所有 Profile 能力同级。`is_default` 只能约束默认记录的路径和删除行为，不能限制官方订阅、供应商、路由、故障转移、会话、MCP、Skill 或用量功能。
2. 任一 Home 相关读写都必须由 `profile_id` 解析目标路径，不能在多 Profile 链路中隐式调用全局 `get_codex_config_dir()`。
3. 每个启用路由的 Profile 恰好拥有一个固定监听端口；一个端口最多属于一个 Profile。
4. 每个 RouteRuntime 独享供应商选择快照、故障转移、熔断器、`CodexChatHistoryStore`、状态统计和日志命名空间。
5. 供应商定义和无状态 HTTP Client/连接池可以共享；请求正文、Header、转换状态、SSE 缓冲和跨请求历史不得跨 Profile 共享。
6. Profile 只引用供应商 ID，不覆盖供应商密钥。需要不同密钥时创建不同供应商记录。
7. Home 内的 `auth.json` 永远属于该 Home。CC Switch 不从其他 Home 复制认证，也不把本地路由凭证转发给上游。
8. 删除 Profile 只删除 CC Switch 绑定和运行时，不删除 Home 或其中任何文件。

## 用户体验

### Codex 页面上下文

Codex 页面顶部新增 Home 上下文栏，显示：

- Profile 名称；
- 规范化后的 Home 路径；
- 当前模式：官方/直连/路由；
- 路由端口与运行状态；
- Profile 管理入口。

用户先选择 Home。选择后，现有供应商列表、当前供应商、路由开关、故障转移、配置/模型、备份、会话、用量、日志、MCP 和 Skill 统一切换到该 Profile。切换期间不展示上一个 Profile 的 current/status 数据；加载完成前显示明确的局部 loading 状态。

供应商卡片仍代表全局供应商定义，可以附带“被哪些 Home 使用”的摘要。用户可以在当前 Home 中启用，也可以通过“应用到其他 Home”批量建立引用，但每次最终写入都必须携带明确的目标 `profile_id`。

### Profile 管理器

管理器以表格展示：名称、Home 路径、模式、端口、当前供应商、路由状态和错误状态，并支持：

- 添加自定义 Home；
- 重命名 Profile；
- 为自定义 Profile 重新绑定路径；
- 修改未运行或安全重启后的路由端口；
- 删除自定义 Profile 绑定；
- 打开/定位 Home 目录。

内置默认 Profile 指向 `~/.codex`，不能删除或改路径，但其他能力与自定义 Profile 完全相同。

### 路由模式

每个 Profile 可独立选择：

- 官方或普通直连：RouteRuntime 不运行，Home 使用自身登录态或供应商配置；
- 路由：Home 的 live 配置指向该 Profile 的本地端口；
- 路由 + 故障转移：只使用该 Profile 自己的候选列表和健康状态。

一个典型结果是：

- `~/.codex` 使用官方订阅；
- `~/codex-work` 在 `127.0.0.1:15722` 路由到供应商 A；
- `~/codex-test` 在 `127.0.0.1:15723` 路由到供应商 B。

这只是配置示例，不构成默认 Profile 与其他 Profile 的权限限制。

## 数据模型

数据库 schema 从 v11 升级到 v12。

### `codex_profiles`

| 字段 | 说明 |
|------|------|
| `id TEXT PRIMARY KEY` | 稳定随机 ID，不从路径直接派生。 |
| `name TEXT NOT NULL` | 用户可修改显示名称。 |
| `home_path TEXT NOT NULL` | 用户选择的展示路径。 |
| `canonical_home_path TEXT NOT NULL UNIQUE` | 规范化后用于唯一性检查的路径。 |
| `is_default INTEGER NOT NULL DEFAULT 0` | 仅表示内置生命周期。 |
| `current_provider_id TEXT NULL` | 对全局 Codex provider 的引用；官方状态可为空或引用现有官方记录。 |
| `current_provider_app_type TEXT NOT NULL DEFAULT 'codex'` | 固定为 `codex`，与 provider ID 组成现有 `providers` 复合外键。 |
| `created_at INTEGER NOT NULL` | 创建时间。 |
| `updated_at INTEGER NOT NULL` | 修改时间。 |

### `codex_profile_routes`

| 字段 | 说明 |
|------|------|
| `profile_id TEXT PRIMARY KEY` | 对 `codex_profiles.id` 的一对一引用。 |
| `enabled INTEGER NOT NULL DEFAULT 0` | 用户期望的持久化启用状态。 |
| `listen_host TEXT NOT NULL` | 初期固定为 loopback。 |
| `listen_port INTEGER NOT NULL UNIQUE` | Profile 专属端口。 |
| `auto_failover_enabled INTEGER NOT NULL DEFAULT 0` | Profile 级故障转移开关。 |
| `max_retries INTEGER NOT NULL` | Profile 级策略。 |
| `request_timeout_seconds INTEGER NOT NULL` | Profile 级策略。 |
| `live_backup_json TEXT NULL` | 接管前配置备份，替代 `app_type` 单例备份。 |
| `live_config_fingerprint TEXT NULL` | 检测外部修改。 |
| `last_error TEXT NULL` | 最近一次 Profile 级启动/恢复错误。 |
| `updated_at INTEGER NOT NULL` | 修改时间。 |

Profile 专属本地鉴权 token 不进入数据库。`CodexProfileSecretStore` 使用稳定 Profile ID 将 token 保存到 CC Switch 本机秘密目录，例如 `~/.cc-switch/secrets/codex-profiles/<profile-id>.token`，并在平台支持时收紧为仅当前用户可读写。该目录不进入数据库导出、WebDAV/S3 同步或普通诊断日志。旧版已经接管的 Profile 首次迁移时写入兼容值 `PROXY_MANAGED`，因此不用改写 Home；用户后续重新激活或主动轮换时再生成随机 token，并通过配置事务同步更新 live 配置。

### `codex_profile_failovers`

| 字段 | 说明 |
|------|------|
| `profile_id TEXT NOT NULL` | Profile 引用。 |
| `provider_id TEXT NOT NULL` | 全局 Codex provider 引用。 |
| `provider_app_type TEXT NOT NULL DEFAULT 'codex'` | 固定为 `codex`，用于复合外键与类型约束。 |
| `position INTEGER NOT NULL` | 有序候选位置。 |

主键为 `(profile_id, provider_id)`，并为 `(profile_id, position)` 建唯一约束。

### 其他 Profile 归属

- 现有 MCP/Skill 定义继续全局共享；新增 `codex_profile_mcp_servers(profile_id, mcp_server_id)` 与 `codex_profile_skills(profile_id, skill_id)` 关联表表达 Profile 级启用关系。旧 `enabled_codex` 只作为迁移输入和非 Profile 兼容字段。
- `proxy_request_logs` 增加可空 `profile_id`，`usage_daily_rollups` 将 `profile_id` 纳入聚合主键，`session_log_sync` 增加可空 `profile_id`。Codex 会话和用量查询必须携带 `profile_id`；旧 Codex 数据在启动迁移时归到旧实际 Home 对应的 Profile，非 Codex 数据保持空值。
- `proxy_live_backup` 的 Codex 单例记录迁移到 `codex_profile_routes.live_backup_json` 或等价 Profile 表；非 Codex 记录保持现状。

所有外键删除策略必须显式：Profile 的 Route/Failover/关联关系可级联删除；被 Profile 引用的全局 provider 不允许删除。

## 后端组件

### `CodexProfileRepository`

只负责 Profile、Route 和 Failover 的持久化与查询：

- 创建/更新/列出 Profile；
- 校验规范化路径唯一性；
- 分配并持久化端口；
- 查询 provider 被哪些 Profile 使用；
- 提供幂等旧数据迁移。

它不读写 Home 文件，也不启动监听器。

### `CodexHomeConfigService`

所有方法显式接收已解析的 `CodexProfile` 或规范化 Home 路径：

- 校验目录存在性、类型、可写性和符号链接结果；
- 读取 live 配置并计算指纹；
- 构造路由接管配置；
- 在同目录写临时文件并原子替换；
- 备份与恢复配置/模型目录；
- 为会话、用量、MCP、Skill 等服务提供显式 Profile 路径解析。

它不读取全局 `codex_config_dir` 来决定目标 Home。

### `CodexProfileSecretStore`

只负责 Profile 本地监听 token 的创建、读取、轮换与删除。token 文件路径由经过校验的稳定 Profile ID 推导，不接受任意用户路径；写入使用原子替换并收紧文件权限。删除 Profile 时只删除这一份 CC Switch 自有秘密文件，不触碰 Home。

### `CodexRouteManager`

应用级单例，维护 `profile_id -> RouteRuntime` 映射，并提供：

- 启动、停止、排空、重启单个 runtime；
- 查询单个或全部 runtime 状态；
- 启动时恢复数据库中期望启用的 Profile；
- 保证一个 Profile 同时最多执行一个生命周期操作；
- 保证一个端口最多被一个 runtime 占用。

它与现有通用 `ProxyService` 并列，不复用其单个 `Option<ProxyServer>` 去覆盖 Claude/Gemini 状态。

### `RouteRuntime`

listener 创建时绑定不可变 `CodexProfileScope { profile_id, home_path, port }`。每个 runtime 独享：

- provider router 的当前选择与 failover 状态；
- circuit breaker；
- `CodexChatHistoryStore`；
- in-flight 计数和 drain 信号；
- runtime status；
- Profile 日志/body dump 命名空间。

可以共享全局 provider repository 和无状态 HTTP Client。每个请求在进入 handler 时取得当前 provider 配置快照，随后整个请求和 SSE 生命周期使用该快照，不被并发切换修改。

## 端口与本地鉴权

- 旧版已接管 Codex 的实际 Profile 保留 `15721`。
- 新 Profile 从 `15722` 开始向上搜索候选端口。
- 自动分配时同时检查数据库保留和操作系统 bind 能力，成功保存后固定该端口。
- 用户可手动修改端口；保存前检查范围、Profile 重复和系统占用。
- 已运行 Profile 修改端口时执行受控重启，并在写入 live 配置前确认新 listener 健康。
- 端口冲突不得静默换号，因为 Home 的 `config.toml` 必须与实际 listener 一致。
- listener 只绑定 loopback，并验证 Profile 专属 Bearer token。
- token 只用于本地 CC Switch listener，转发前移除或替换为目标供应商授权头。

## 激活、切换与关闭

### 激活路由

1. 读取 Profile、Route 和目标 provider，并校验引用完整。
2. 获取该 Profile 的操作锁。
3. 校验 Home、端口、本地凭证和外部配置冲突。
4. 构造无副作用的 activation plan，包含旧数据库状态、旧 runtime 状态、live 配置指纹和目标配置。
5. 启动目标 RouteRuntime，并通过本地健康检查确认可接收请求。
6. 备份目标 Home 的 live 配置。
7. 在 Home 同目录原子写入路由配置与模型目录更新。
8. 最后提交 Profile 当前 provider、route enabled、备份和指纹。
9. 任一步失败，按相反顺序恢复文件、runtime 和数据库状态，并保留可诊断错误。

这样不会出现“配置已指向端口，但 listener 没启动”的可见中间状态。

### 切换供应商

- 保持 Profile 端口、本地 token 和 live 配置不变。
- 校验新 provider 后，原子替换 runtime 的 provider 路由快照，再持久化 `current_provider_id`。
- 已进入 handler 的请求继续使用旧快照；新请求使用新快照。
- 若持久化失败，恢复旧快照。
- 切换 Profile A 不修改 Profile B 的 router、breaker 或 failover。

### 关闭路由

1. 获取 Profile 操作锁并阻止新的生命周期变更。
2. 检查 live 配置指纹，避免覆盖用户外部修改。
3. 原子恢复该 Profile 接管前配置。
4. runtime 停止接收新请求并在超时内排空在途请求。
5. 停止 listener，持久化 disabled 状态。
6. 任何恢复失败都保留 Profile 和可重试状态，不删除绑定。

## Profile 生命周期

### 添加

- 展开 `~`、转成绝对路径并尽可能解析符号链接得到 canonical path。
- 拒绝空路径、重复 canonical path、普通文件、不可访问目录和不可写目录。
- 不要求该目录必须是默认 Home，也不限制其使用官方订阅。
- 创建 Profile 与 Route 默认记录，并从 `15722` 起分配端口。
- 添加过程不复制 `auth.json`，不主动接管配置。

### 重命名与重新绑定

- 重命名只改 CC Switch 显示名称。
- 默认 Profile 不允许重新绑定。
- 自定义 Profile 只有在路由关闭且不存在未完成恢复时才能重新绑定。
- 重新绑定先验证新路径唯一且可用，再原子更新；不移动旧目录内容。

### 删除

- 默认 Profile 不可删除。
- 自定义 Profile 若路由已启用，先执行关闭/恢复；失败则中止删除。
- 删除 runtime、关联关系和 Profile 数据，不删除 Home 目录或其文件。
- 删除供应商是反向约束：若任一 Profile 引用它，则阻止删除并返回使用它的 Home 列表。

## 旧版本迁移

迁移由两个连续阶段组成：数据库 v11 -> v12 只创建 Profile 表、关联表和归属字段；应用启动时的 `CodexProfileMigrationService` 在数据库与旧设置都可用后填充数据。整个迁移必须幂等：

1. 创建指向 `~/.codex` 的默认 Profile。
2. 读取旧 `codex_config_dir` override：
   - 未设置或等于 `~/.codex`：旧状态归到默认 Profile；
   - 指向其他目录：额外创建“原 Codex 配置”Profile，并保持它为当前选中 Profile。
3. 把旧 `current_provider_codex`、Codex failover、路由启用状态、live backup 和端口 `15721` 迁到“旧版本实际使用目录”对应的 Profile。
4. 旧版已接管时，升级不恢复或重写 live 配置；新 manager 按迁移后的 enabled 状态接续 listener。
5. 旧单例字段只作为兼容读取和迁移标记，不再接收新写入。
6. 不移动、不复制、不改写任何 Home 文件。

重复启动或重复执行 migration 不得创建重复 Profile、重复端口或覆盖用户已修改的新数据。

## 错误与并发处理

- Profile 级操作锁串行化同一 Home 的激活、切换、改端口、关闭和删除；不同 Profile 可并发。
- 路径和端口校验必须在副作用前执行。
- runtime panic/意外退出只把对应 Profile 标为 error，并保留其他 listener。
- 配置写入失败必须恢复同一 Home 的旧文件，不触碰其他 Home。
- 发现 live 配置指纹不一致时，不静默覆盖；UI 提供重新读取、放弃 CC Switch 备份或明确覆盖等后续选择，本期至少实现安全停止与错误提示。
- 应用退出时并发排空所有 runtime；单个 runtime 超时不阻塞其他 runtime 的清理。
- 启动恢复时逐 Profile 尝试，失败 Profile 记录错误但不阻止其他 Profile 启动。

## 安全与隐私

- Home 路径在 API 边界规范化，文件操作只使用 repository 返回的 Profile，不接受 handler 自由拼接路径。
- 防止同一路径通过 `~`、`..` 或符号链接被重复登记。
- 新 Profile 的 local token 使用密码学安全随机值；旧接管 Profile 在不改写 Home 的首次迁移中允许保留兼容 token，后续可事务化轮换。UI 默认不展示明文。
- 日志对 API Key、`auth.json` 内容和 local token 脱敏。
- 请求转发前删除 CC Switch local token，测试证明它不会出现在上游 Header。
- Profile A 的错误、响应片段和 body dump 不得写入 Profile B 的命名空间。

## 前端状态与缓存

- 增加稳定的 `selectedCodexProfileId` 设置；删除当前 Profile 时回退到默认 Profile。
- Codex Profile 相关查询 key 至少包含 `profileId`，例如 `['codex-profile-state', profileId]`。
- 供应商定义列表仍可按 `appId='codex'` 全局缓存；“当前供应商/路由状态/引用关系”是 Profile 级查询。
- 切换 Profile 时取消或隔离旧 Profile mutation 的乐观更新；旧请求晚到不能覆盖当前视图。
- 非 Codex 页保持现有 `activeApp` 和单例 current provider 语义。

## 可观测性

- runtime 状态至少包含 `stopped`、`starting`、`running`、`draining`、`error`。
- 日志结构字段包含 `profile_id`、Profile 名称、监听端口、request_id；不记录秘密。
- `CC_SWITCH_DUMP_BODY=1` 时写入 `~/.cc-switch/logs/proxy-bodies/<profile-id>/...` 或等价 Profile 目录。
- Profile 管理器展示最近错误，并允许重试单个 Profile。

## 测试策略

### 数据与迁移

- 默认 Profile 正确创建且不能删除/改路径。
- 自定义 Profile 路径规范化、符号链接重复和端口唯一性。
- v11 默认目录与 override 目录两种迁移。
- 迁移重复执行不重复写入。
- 所有 Profile 均可选择官方订阅/官方供应商。

### 配置事务

- 激活成功、listener 启动失败、备份失败、原子写入失败、数据库提交失败的回滚。
- 关闭时恢复失败不删除 Profile。
- 外部修改指纹冲突不被静默覆盖。

### 请求隔离

- Profile A/B 在不同端口并发发送不同正文，分别命中绑定 provider。
- A/B 故意使用相同 `session_id`、`response_id` 和 `call_id`，工具历史仍完全独立。
- A 流式请求期间切换 provider，A 在途请求保持旧快照，B 不受影响。
- A 触发熔断/failover 不改变 B 的 breaker 和当前 provider。
- A runtime 崩溃或端口冲突不停止 B。
- local token 在 listener 验证后不会转发上游。

### 前端

- 快速切换 Profile 不闪现上一个 Home 的 current/status。
- 所有 Codex mutation 携带当前 `profileId`。
- provider 卡片正确显示引用它的 Home。
- 删除被引用 provider 时展示 Home 列表。
- 默认与自定义 Profile 展示相同功能入口。

## 验收标准

- 用户可登记默认 `~/.codex` 和多个自定义 Home，并在 Codex 页面先选择 Home。
- 至少两个 Profile 可分别在 `15722`、`15723` 等独立端口同时运行路由。
- 两个实例同时发送请求时，正文、工具历史、供应商选择、failover 和日志互不交叉。
- 任意 Profile 都能使用其自己的官方订阅或选择路由，不受 `is_default` 限制。
- 供应商定义全局共享，同一供应商可被多个 Home 引用。
- 删除 Profile 不删除 Home；删除被引用 provider 会被安全阻止。
- 单个 Profile 的路径、端口、runtime 或配置错误不影响其他 Profile。
- 旧单 Home 数据自动、幂等迁移，既有 `15721` 接管保持可用。
- 前端所有 Profile 级查询/修改都使用 `profile_id`，切换时不显示串数据。

## 已否决方案

### 继续使用单一全局 `CODEX_HOME`

只能在一次操作中切换目录，无法让多个实例同时保持不同路由与供应商状态。

### 单端口 + Profile token 分流

技术上可行，但所有 runtime 状态仍容易被放回一个共享 server，隔离契约依赖每个跨请求容器都正确分区。用户明确选择独立路由；一 Home 一端口能从 listener 生命周期开始建立结构隔离，更符合“内容不能串”的首要要求。

### 为每个 Home 复制供应商

会重复 API Key、模型配置和维护动作，也无法表达一个供应商被多个 Home 共享。采用全局定义 + Profile 引用更清晰。

### 只有默认 Profile 能用官方订阅

这不符合 Codex 的 `CODEX_HOME` 语义，也被用户明确否决。每个 Home 都可以拥有自己的 `auth.json` 和官方订阅。
