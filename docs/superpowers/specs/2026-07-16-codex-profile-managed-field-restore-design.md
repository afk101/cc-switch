# Codex Profile 路由字段级恢复设计

## 状态

- 设计日期：2026-07-16
- 状态：已批准
- 关联 Findings：`docs/superpowers/findings/2026-07-16-codex-api-route-disable-fingerprint-findings.md`
- 适用范围：Codex Profile 独立 `CODEX_HOME` 路由关闭与未完成关闭补偿

## 背景

Codex Profile 路由启用时，CC Switch 会把目标 Home 的 `config.toml` 中三个请求传输字段改为本地监听器配置：

- `base_url`
- `wire_api`
- `experimental_bearer_token`

现有实现同时保存接管前正文、接管前整文件指纹和接管目标整文件指纹。关闭路由时只有当前整文件指纹仍等于接管目标时才允许恢复接管前正文。

Codex Desktop 会在运行期间正常更新同一个 `config.toml`，例如写入 `js_repl`、plugins、MCP、marketplaces、desktop 等设置。这些更新不会改变路由身份，却会改变整文件指纹，导致关闭进入 `disable_home_restore_failed`。后续操作会先重放同一补偿记录，因此稳定重复失败。

## 目标

1. 关闭路由时只验证 CC Switch 真正拥有的三个路由字段。
2. 验证成功后只恢复这三个字段的接管前状态。
3. 保留关闭瞬间所有其他配置，包括用户最新选择的 `model`。
4. 三个路由字段被真正修改时仍拒绝覆盖。
5. 正常关闭与 pending disable recovery 使用同一套幂等逻辑。
6. 不修改数据库 schema 或 `live_backup_json` 序列化格式。
7. 保持每个 `CODEX_HOME`、端口和 listener token 的 Profile 隔离。

## 非目标

- 不修改原有全局 `ProxyService` 的关闭语义。
- 不改变路由启用、供应商转发、故障转移或认证协议。
- 不修改前端 UI。
- 不自动修复无效 TOML。
- 不把 token 明文或摘要新增到数据库。
- 不直接修改用户现场 `~/.codex-api`；现场由升级后的正常补偿流程收敛。

## 字段语义

| 字段 | 路由接管值 | 用途 | 关闭策略 |
|------|------------|------|----------|
| `base_url` | `http://127.0.0.1:<Profile 端口>/v1` | 将 Codex 请求送到该 Profile 的独立监听器 | 当前仍为本地目标时恢复接管前值或删除 |
| `wire_api` | `responses` | 指定 Codex 使用 OpenAI Responses API 协议 | 当前仍为 `responses` 时恢复接管前值或删除 |
| `experimental_bearer_token` | 该 Profile 的 listener token | 生成访问本地监听器的 `Authorization: Bearer`；代理验证后在上游转发前移除 | 当前仍匹配 listener token 时恢复接管前值或删除；错误信息永不展示 token |

`model`、`model_provider` 和所有其他字段均不属于关闭时的严格所有权判断。它们使用关闭瞬间的当前值。

## 架构

### `CodexHomeConfigService`

Home 配置服务负责全部 TOML 字段级逻辑：

- 解码既有 `CodexRouteBackup`。
- 从 `previous_content` 构造不包含 provider model 改写的纯路由目标。
- 分别推导三个严格字段的准确 TOML 路径。
- 记录纯路由构造相较 previous 新建的 provider 子表与父表，仅供恢复后清理空结构。
- 读取 previous、target、current 三态，并保留字段“缺失/存在”及原始 TOML Item。
- 判断当前是路由目标态、已经恢复态还是真正冲突态。
- 以 current 文档为基底，仅恢复三个字段。
- 写入前重新读取并校验本轮短事务指纹，再执行原子替换或删除。

服务不读取数据库、运行时或默认 `CODEX_HOME`。

### `CodexRouteManager`

route manager 负责生命周期编排：

- 在 Profile 锁内保存/恢复 lifecycle operation。
- 读取既有 listener token，不因关闭操作创建新 token。
- 将显式 Home、Profile 端口、backup 和 token 交给 Home 配置服务。
- Home 收敛后排空请求、停止独立监听器并提交禁用状态。

正常 disable 和 pending disable recovery 必须调用同一个 manager 私有恢复入口。

### `CodexProfileTokenStore`

token store 契约增加只读方法，返回 `Option<String>`：

- 启用与需要补建凭证的既有流程继续使用 `ensure_token`。
- 关闭与关闭补偿只使用 `read_token`。
- token 缺失时无法证明路由所有权，保留补偿记录并返回脱敏错误。

## 字段路径推导

三个字段不保证位于同一层级：

- `base_url`、`wire_api` 在有 `model_provider` 时写入对应 provider 表，否则写在顶层。
- 自定义 provider 的 `experimental_bearer_token` 优先位于 provider 表。
- Codex 保留 provider ID 的 token 可能位于顶层。

实现不得重新复制 provider ID 判断规则。应执行：

1. 将 `previous_content` 解析为 previous 文档；缺失文件视为空文档。
2. 使用现有 `build_codex_profile_route_toml(previous, port, None, listener_token)` 纯构造 target 文档。传入 `None` 保证不改写 `model`。
3. 在 target 文档中分别定位值等于本地 URL、`responses` 和 listener token 的字段路径。
4. 使用相同路径读取 previous 与 current 的值。

字段路径只属于本次接管位置。即使关闭期间 `model_provider` 改变，也不扩大 CC Switch 对其他 provider 表的所有权。

## 三方恢复算法

输入：

- `previous`：`live_backup_json.previous_content`
- `target`：从 previous、Profile 端口和 listener token 纯构造
- `current`：关闭瞬间读取的配置

处理：

1. 解析 previous 与 target；previous 缺失时使用空文档。读取 current：无效 TOML 返回明确错误；文件缺失时只有 previous 文件原本也不存在才视为合法幂等结果，否则按外部删除冲突处理。
2. 推导三个 target 字段路径和期望值。
3. 在这些路径读取 previous 和 current 字段状态；字段状态区分“缺失”和“存在”，存在时保留原始 `toml_edit::Item`，不得把非字符串值折叠为缺失。
4. 如果 current 文件存在且三字段状态整体等于 previous，判定 Home 已恢复，幂等返回，不写文件；current 与 previous 都不存在也直接幂等返回。
5. 否则逐项验证 current 是否等于 target：
   - `base_url` 必须等于该 Profile 本地 URL。
   - `wire_api` 必须等于 `responses`。
   - token 必须等于 listener token。
   - 迁移期旧占位符继续兼容现有规则：仅当 base URL 与端口匹配且 wire API 匹配时允许 `PROXY_MANAGED`。
6. 任一字段不匹配立即返回字段级冲突，不修改文件。
7. 以 current 文档为输出基底，将三个路径分别设置为 previous 的原始 Item；previous 缺失的字段从 current 删除。
8. 只清理由本次路由创建且恢复后为空的 provider/table 结构；当前存在的其他项不删除。
9. 写入前再次读取配置并核对其整文件指纹仍等于步骤 1 的 current 指纹；不一致则返回并发修改错误。
10. 合并结果为空且 previous 文件原本不存在时删除配置文件；否则原子写入合并后的 TOML。

步骤 9 是短事务重校验，用于检测常见的校验后并发写入；它不再把路由运行期间的长期合法变化当成冲突。

## 生命周期与补偿

### 正常关闭

1. 获取 Profile 锁并完成既有 pending operation 恢复。
2. 保存 `disable/prepared` 操作记录。
3. 从 token store 只读 listener token。
4. 执行字段级 Home 恢复。
5. 失败时保存 `disable_home_restore_failed` 与脱敏错误摘要。
6. 成功时排空在途请求并停止 runtime。
7. 保存禁用状态、清理 operation/error 并移除 manager runtime。

### 未完成关闭恢复

`recover_pending_locked` 在 `prepared`、`disable_home_restore_failed` 或其他现有关闭恢复阶段中：

1. 读取同一 Profile 与只读 listener token。
2. 调用相同字段级恢复入口。
3. 如果字段已经等于 previous，直接继续后续 runtime/DB 收敛。
4. 如果字段仍等于 target，先合并再继续。
5. 真正冲突或凭证缺失时保留 recovery，拒绝后续 mutation。

这使当前 `.codex-api` 的 `disable/disable_home_restore_failed` 能在新版本中通过再次关闭或启动恢复自动收敛，无需手工改数据库。

## 错误与安全

- base URL 冲突：报告字段名、期望本地 URL和实际 URL/缺失状态。
- wire API 冲突：报告字段名、期望 `responses` 和实际值/缺失状态。
- token 冲突：只报告“缺失”或“不匹配”，不得包含期望 token、实际 token、其前缀或摘要。
- token 文件缺失：报告无法证明 Profile 路由所有权，不调用 create/ensure。
- previous/current TOML 无效，或 previous 文件存在但 current 文件被删除：不写文件，保留 recovery。
- 短事务指纹变化：报告配置在恢复期间再次变化，不写文件。
- 原子写入/删除失败：保持现有文件操作错误，并保留 recovery。

`auth.json` 不参与读取、备份、合并或写入。

## 数据兼容

- `CodexRouteBackup` 继续只包含 `previous_content`、`previous_fingerprint`、`target_fingerprint`。
- 不增加 SQLite migration。
- 既有 backup 和 recovery JSON 可直接读取。
- `target_fingerprint` 继续用于启动对账、旧流程兼容和诊断，但不再作为 Profile 关闭恢复的长期唯一所有权证明。
- 默认 `~/.codex` 与任意自定义 Home 使用完全相同的逻辑。
- 旧全局 `proxy_live_backup` 和 `ProxyService` 不修改。

## 测试要求

### Home 配置服务

- Codex Desktop 新增非路由段后关闭成功，所有新增段保留。
- 路由期间修改 `model` 后关闭，最新 model 保留。
- previous 三字段存在时恢复原值；不存在时只删除三个字段。
- 顶层与 provider 表路径均覆盖，包括 token 与 base/wire 位于不同层级。
- base URL、wire API、token 各自冲突时拒绝且文件原样保留。
- 任一严格字段存在但不是字符串时按冲突处理，不能误判为字段缺失；previous 的原始 Item 可原样恢复。
- token 错误文本不包含测试 token。
- 已恢复状态重复调用为 no-op。
- 旧 `PROXY_MANAGED` + 同端口仍可恢复。
- current TOML 无效、短事务二次读取变化、原子写失败均不覆盖当前内容。

### Route manager

- 正常 disable 使用只读 token，并保留 Desktop 配置。
- pending `disable_home_restore_failed` 自动继续收敛。
- Home 已恢复但 runtime stop 失败后，再次执行能够完成。
- token 缺失保留 recovery，且 ensure/create 调用次数为零。
- Profile A 关闭不读取或修改 Profile B 的 token、Home 或端口。

## 验收标准

1. 在路由开启后由 Codex Desktop 写入插件/MCP 配置，关闭路由成功且这些配置仍存在。
2. 关闭后 `base_url`、`wire_api`、token 与接管前状态一致。
3. 路由期间最新 `model` 保留。
4. 任一严格字段真正变化时关闭失败且不覆盖当前配置。
5. 当前 `.codex-api` 卡住的补偿记录可以由新版本正常收敛。
6. 不出现 token 明文日志或错误。
7. 所有相关 Rust 测试、格式检查与 diff 检查通过。
