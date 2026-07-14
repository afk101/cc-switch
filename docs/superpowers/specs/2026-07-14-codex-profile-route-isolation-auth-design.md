# Codex Profile 路由开关隔离与本地认证修复设计

## 状态

- 日期：2026-07-14
- 状态：已授权按推荐方案直接实施
- 范围：Codex Profile 顶部路由开关、Profile Home 本地凭证投影、已启用路由启动恢复、旧 Codex 全局接管退役

## 问题与根因

当前同时存在两套 Codex 路由生命周期：

1. 旧应用级 `ProxyToggle -> set_takeover_for_app("codex") -> ProxyService`，没有 Profile ID，读写全局当前供应商和全局 Live 配置。
2. 新 Profile 级 `enable_codex_profile_route -> CodexRouteManager`，按 Profile ID 管理独立 Home、端口、供应商快照和 runtime。

Codex 页面顶部仍渲染第一套开关，因此在 `codex-api` Profile 中点击时，实际读到默认 Profile 的 OpenAI Official，并由旧全局接管向 Home 写入 `PROXY_MANAGED`。

Profile 启用流程又在构建 Home 配置计划之后才生成 43 字节私有 listener token，且配置构建器没有 token 参数，所以 `config.toml` 保留 `PROXY_MANAGED`，listener 却校验随机 token。现场对照已证明：Home token 请求 `/v1/models` 返回 401，listener token 返回 200。

启动期还会根据旧 `proxy_config(codex).enabled` 恢复全局接管，而 Profile 恢复只启动 listener、不校准 Home。因此两个问题会在重启后持续。

## 目标

- Codex 顶部路由开关只作用于当前选中 Profile。
- 切换 Profile 时，开关状态、加载态、错误和 mutation 全部按 Profile ID 隔离。
- 任意 Profile（包括默认 Profile）都可以使用官方订阅、第三方直连或私有路由，不按默认/自定义区分权限。
- listener 和 Home 始终使用同一份 Profile 私有 token。
- 新版启动后能自愈已启用但仍持有 `PROXY_MANAGED` 的 Profile，无需删除 Profile 或 Home。
- 旧 Codex 全局接管不再参与 UI 和启动恢复；Claude/Gemini 原逻辑不变。
- 不读写、复制或删除任何 Profile 的 `auth.json`、会话或其他 Home 数据。

## 非目标

- 不重新设计 Claude/Gemini 的应用级接管。
- 不在本次新建 Profile 级故障转移编辑器；旧全局 `FailoverToggle` 不得再在 Codex 上显示。
- 不让 CC Switch 负责启动或注入多个 Codex Desktop/App。
- 不将 listener token 写入数据库、日志、导出或同步文件。

## 方案比较

### 方案 A：Profile 专用开关 + token 单真值源 + 启动自愈（采用）

- 新建 `CodexProfileRouteToggle`，强制接收选中 Profile 和该 Profile state。
- 使用 Profile 专用 enable/disable mutation，不调用通用 `ProxyService`。
- `ensure_token` 先于配置计划，配置构建器必须接收并写入同一 token。
- 串行退役旧 Codex 全局状态，再恢复并对账 Profile。
- 用指纹和补偿记录保护已启用 Profile 的原始备份与外部修改。

优点是作用域从类型和 API 边界就无法串用，同时修复现有现场。代价是需要补齐启动补偿状态。

### 方案 B：扩展通用 `ProxyToggle` 支持两种模式（不采用）

让同一组件根据 `activeApp === "codex"` 改走 Profile API。这样改动少，但一个组件会同时承担应用级接管与 Profile 级路由两套状态、tooltip、错误和 API，很容易再次回落到全局状态，不符合单一职责。

### 方案 C：只修前端开关和下次手动启用（不采用）

替换开关，并要求用户先关闭再开启路由来重写 token。它不会清除旧全局启动恢复，也不会自愈当前 401；重启后可再次损坏配置，不满足“直接可用”。

## 前端设计

### `CodexProfileRouteToggle`

组件保留现有顶部位置和视觉密度，但只接收 Profile 数据：

```ts
interface CodexProfileRouteToggleProps {
  profileId: string | null;
  state: CodexProfileState | undefined;
  isStateLoading: boolean;
}
```

状态语义：

- `checked = state?.route?.enabled === true`。
- Profile 状态加载中、没有 Profile，或 mutation 进行中时禁用。
- 开启时使用当前 Profile 的 `route.currentProviderId`；缺失时显示明确错误，不猜测全局供应商。
- 关闭时只调用 `disableRoute(profileId)`。
- 成功后只失效 `codexProfileKeys.state(profileId)`。
- tooltip 显示当前 Profile 名称、Home、端口和 runtime 状态，不读全局代理端口。
- 切换 Profile 后，新 query 未完成前开关显示 loading，不保留上一 Profile 的 checked 状态。

`App.tsx` 在 Codex 分支渲染专用开关；Claude/Gemini 继续渲染 `ProxyToggle`。Codex 不再渲染应用级 `FailoverToggle`。ProviderList 的运行/接管表达在 Codex 下也改为来自选中 Profile state，不使用 `takeoverStatus.codex`。

## token 单真值源

`CodexProfileSecretStore` 仍是 listener token 的唯一持久化真值源。启用时：

1. 在持有 Profile 操作锁后调用 `ensure_token(profile_id)`。
2. 将返回 token 作为 `build_profile_route_plan` 必填参数。
3. 配置构建器将它写到当前 model provider 的 `experimental_bearer_token`，替换 `PROXY_MANAGED`、旧 listener token 或旧上游 key。
4. 同一 `String` 交给 runtime factory；不再生成第二份 token。

本地 listener 在请求进入时校验该 token，随后 Provider adapter 删除/替换本地授权头，上游仍使用全局供应商自己的 API key。`auth.json` 不参与该流程。

## 已启用 Profile 的启动对账

启动恢复不再只启动 runtime。对每个 `route.enabled=true` 的 Profile：

1. 恢复未完成的生命周期记录。
2. 确保 token，用 Profile 端口、供应商和 token 重建目标配置。
3. 若 Home 已与目标指纹一致，直接启动 runtime。
4. 若 Home 与旧 Profile 目标指纹一致，或可明确识别为已退役的 Codex 全局占位配置，则执行可恢复对账。
5. 其他指纹视为用户/外部修改，不覆盖，不启动 runtime，并把隔离错误写入该 Profile。

对账使用专用 recovery operation，只记录旧/新指纹和阶段，不记录 token 或 Home 正文。崩溃后根据 Home 是旧目标还是新目标来决定重做或提交：

- Home 仍是旧目标：重新构建并应用当前 token 目标。
- Home 已是新目标：更新 route backup 的 `target_fingerprint` 并清除 recovery。
- Home 两者都不是：保留 recovery 并报外部冲突。

新 backup 始终保留最初的 `previous_content` 和 `previous_fingerprint`，只更新 `target_fingerprint`，保证之后关闭路由仍恢复真正的路由前配置。

## 旧 Codex 全局接管退役

退役必须在 Profile 恢复之前完成，并且与通用崩溃恢复串行：

- 若存在已启用 Codex Profile，该 Home 由 Profile 生命周期管理。仅清理旧 Codex `proxy_config.enabled/live_takeover_active` 和全局 backup，不盲目用全局 backup 覆盖 Profile Home；随后由 Profile 对账修正 Home。
- 若没有已启用 Profile，先走旧关闭流程恢复 Live 配置，成功后再清理旧状态。
- 通用 `restore_proxy_state_on_startup` 之后只枚举 Claude/Gemini，不再枚举 Codex。
- 退役失败时不并发启动 Profile runtime，避免两个 writer 同时改 Home；错误记日志并留待下次启动重试。

## 不变量

1. 任意一次 Codex 启停 mutation 都必须携带非空 Profile ID。
2. Codex 页面不再读取或写入 `takeoverStatus.codex`。
3. listener token 绝不进入数据库、recovery JSON、普通日志或 body dump。
4. Home 配置中的 listener token 与密钥文件字节完全一致。
5. 启动恢复遇到不可识别的外部 Home 修改时宁可停止该 Profile，也不覆盖用户文件。
6. 一个 Profile 恢复失败不阻断其他 Profile 恢复。
7. 默认 Profile 只在路径重绑和删除上有限制，路由/订阅能力与其他 Profile 相同。

## 错误与可观测性

- UI 错误必须指向当前 Profile，不再发出旧全局官方供应商警告。
- 启用、退役、对账、runtime 启动和补偿各有中文结构化日志，只记 Profile ID、Home、端口、指纹缩略和阶段，不记 token。
- `last_error` 只写入失败 Profile，成功恢复后清除。
- body dump 仍按 Profile ID 目录隔离；本地认证失败发生在 handler 前，不得产生含凭证的 body 日志。

## 测试与验收

### 自动化验收

- 前端组件测试证明 A/B Profile 的 checked 状态、enable/disable 参数和 query invalidation 不串用。
- App 接线测试证明 Codex 使用专用开关，Claude/Gemini 仍使用旧开关，Codex 不渲染全局 failover 开关。
- Rust 配置测试证明 `PROXY_MANAGED` 被精确 listener token 替换，并且只写目标 Home。
- manager 测试证明 token 在计划前生成，Home/listener 同值，对账崩溃可收敛，外部修改不被覆盖。
- 启动测试证明旧 Codex 状态被退役，Claude/Gemini 恢复不变。

### 真实闭环

1. 使用 `pnpm run dev:dump` 启动新构建。
2. 用 Computer Use 选择默认 Profile，确认其官方订阅状态和路由开关不受 `codex-api` 影响。
3. 切换到 `/Users/qihoo/.codex-api`，确认顶部开关反映该 Profile 的 enabled 状态，关闭/开启均只改该 Profile。
4. 请求 `http://127.0.0.1:15722/v1/models` 和真实 `/v1/responses`，确认不再返回本地凭证 401。
5. 对比 Home token 与 listener token 的脱敏哈希，确认一致；检查 `~/.cc-switch/logs/proxy-bodies/<profile-id>` 和应用日志，确认 Profile 归属、上游转发与错误为空。
6. 重启 `pnpm run dev:dump`，重复核心检查，证明旧全局状态不会再回写。
