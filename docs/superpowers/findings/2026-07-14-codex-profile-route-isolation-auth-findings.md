# Codex Profile 路由隔离与本地认证调查

## 需求

- 默认 Profile 使用官方订阅时，新建 `/Users/qihoo/.codex-api` Profile 并开启路由，不得提示或修改默认 Profile；路由开关和状态必须按 Profile 隔离。
- `/Users/qihoo/.codex-api` 通过 `http://127.0.0.1:15722/v1/responses` 使用路由时，不得出现“Codex Profile 本地监听凭证无效”的 401。
- 根因明确后采用推荐方案，完成实现、自动化回归，并使用 `pnpm run dev:dump` 与 Computer Use 验证真实闭环。

## 用户提供的第一手信息

- 默认 Profile 当前使用官方订阅。
- 新建 Profile 的 Home 为 `/Users/qihoo/.codex-api`，该 Profile 使用路由。
- 开启路由时出现截图 1 的提示；用户判断该操作错误影响了默认 Profile。
- 配置完成后，Codex 请求 `http://127.0.0.1:15722/v1/responses` 返回 `unexpected status 401 Unauthorized: 认证失败: Codex Profile 本地监听凭证无效`。

## 研究发现

- 当前 Git 分支为 `feature/v3.16.5`，业务工作区无未提交改动；只有本调查 findings 为新文件。最近提交包含 Profile 管理、独立停止路由和 App 测试夹具修复。
- 数据库只有两个 Profile：默认 `codex-default` 使用 `/Users/qihoo/.codex`、端口 15721、路由禁用；`codex-api` 使用 `/Users/qihoo/.codex-api`、端口 15722、路由启用。两者 `current_provider_id` 不同，说明后端路由记录本身已经按 Profile 分行存储。
- `/Users/qihoo/.codex-api/config.toml` 当前写入 `model_provider = "custom"`、`wire_api = "responses"`、`requires_openai_auth = true`、`base_url = "http://127.0.0.1:15722/v1"`，并存在已脱敏的 `experimental_bearer_token`；本地路由配置表面完整。
- `/Users/qihoo/.codex-api` 是实际使用中的完整 Codex Home，包含 `auth.json`、会话、状态库和桌面日志，不是空测试目录；后续验证与修复不得删除或覆盖其用户数据。
- Memory 注册表没有本仓库或本缺陷的可用历史记录，后续以当前代码、数据库、配置和日志为准。
- 本地认证失败已有直接证据：`config.toml` 中的 `experimental_bearer_token` 长度为 13，而 Profile listener token 文件长度为 43；两者 SHA-256 不同，`token_match=no`。listener token 文件权限正确为 `0600`，因此不是文件缺失或权限问题，而是配置写入了错误/陈旧凭据。
- Provider ID 映射确认：默认 Profile 的 `9c15dc2e-...` 是 `OpenAI Official`；`codex-api` 的 `4847cf65-...` 是 `claude-openai-chat`。截图告警读取到默认 Profile 的官方供应商，而同屏 `codex-api` 卡片已经选中第三方供应商，证明至少告警判断的数据源跨了 Profile 边界。
- 时间线：listener token 在 17:21:15 创建，同时 15722 listener 启动；`/Users/qihoo/.codex-api/config.toml` 在 17:24:52 后续写入。错误配置不是“listener 启动后 token 又轮换”，而是较晚的 Home 配置写入阶段没有使用该 listener token。
- `/Users/qihoo/.codex-api/logs/desktop.log` 至少两次记录完全相同的 401，均由桌面端真实请求 `http://127.0.0.1:15722/v1/responses` 触发；Profile 对应的 proxy body 目录没有请求日志，符合请求在本地 listener 认证层即被拒绝、尚未进入上游转发的时序。
- UI 告警文案来自 `notifications.proxyOfficialWarning`，调用点位于 `src/App.tsx` 约 499 行；顶部开关仍关联全局 `useProxyStatus` / `start_proxy_server` / `stop_proxy_with_restore` 链路，而 Profile 路由启用来自独立的 `enable_codex_profile_route`。这两套开关语义并存，是继续调查的核心边界。
- 后端 Profile 启用路径位于 `CodexProfileRouteManager`：先由 `CodexHomeConfigService` 构建 Home 配置计划，再调用 `secret_store.ensure_token(profile_id)` 获取 token，随后启动 listener 并应用配置计划。必须核对构建计划和 ensure token 的先后关系及 token 是否进入计划。
- 认证调用链的代码缺陷已定位：`enable()` 在构建 `CodexRouteConfigPlan` 之后才执行 `ensure_token(profile_id)`；`build_profile_route_plan()`/`build_codex_profile_route_toml()` 的参数只有 Home、端口、Provider，函数只更新 `base_url`、`wire_api` 和可选 `model`，完全没有写入 Profile listener token。因此现有 `experimental_bearer_token` 会原样保留，恰好解释配置里 13 字节旧值与新 listener 的 43 字节随机值不一致。
- listener 本身使用 `ensure_token()` 返回的 43 字节 Profile 私有 token 启动，校验逻辑没有读取 Home 配置；所以 401 是“配置投影漏写 token”，不是 listener 比较算法错误。
- App 的 ProviderList 已正确用 `codexProfileState.route.currentProviderId` 渲染卡片选中态；与此同时，全局代理 `useProxyStatus` 仍控制应用头部开关与 `proxy-official-warning` 事件。UI 同一页面混用了 Profile 状态和应用级全局代理状态。
- 顶部开关组件已确认是 `ProxyToggle`：不接收 `selectedCodexProfileId` 或 `codexProfileState`，只调用 `set_proxy_takeover_for_app({ appType: "codex" })`，其 checked 状态来自全局 `takeoverStatus.codex`。因此它不是 Profile 路由开关，却在 Codex Profile 页面被无条件显示，正是用户看到“影响默认 Profile”的入口。
- `ProxyToggle` 的说明仍是“接管 Codex 的 Live 配置”，后端告警事件来自旧全局 `services/proxy.rs`；它会读取全局 Codex 当前供应商（默认 Profile 的 OpenAI Official），与当前选中的 `codex-api` Profile 无关。截图 1 不是误报文案，而是错误调用了另一套全局接管功能。
- 后端 `set_proxy_takeover_for_app("codex", true)` 明确执行旧的全局流程：备份全局 Codex Live 配置、同步全局当前供应商、写全局接管配置、设置应用级 `proxy_config.enabled`，最后通过 `get_effective_current_provider` 对全局官方供应商发告警。它从接口层就没有 Profile ID，无法满足 Profile 私有语义。
- 旧多 Home 设计文档已经明确要求“用户选择 Home 后，现有供应商列表、当前供应商、路由开关……统一切换到该 Profile”，并要求 Profile 专用 runtime 避免复制全局接管逻辑；当前 `ProxyToggle` 接线遗漏违反了既有设计，不是需求新增。
- 现有 Home 配置测试只围绕端口、wire API、模型和配置冲突，没有覆盖“写入与 listener 完全相同的 Profile token”，所以配置投影漏字段仍能通过测试。
- `home_config` 现有四个测试覆盖显式 Home 隔离、原子写失败、外部修改冲突和篡改路径防护，但没有断言 `experimental_bearer_token`；前端也没有 `ProxyToggle` 的 Codex Profile 作用域测试。
- 当前真实 CC Switch 进程仍在 `127.0.0.1:15722` 监听，数据库 route 也为 enabled；问题可在现有现场继续做非破坏性认证对照，不需要先修改配置或重启服务。
- Profile router 的 `/v1/models` 经过与 `/v1/responses` 相同的本地 Bearer middleware，但不会请求上游，适合无副作用认证对照。现场实测：使用 Home `config.toml` 中的 token 返回 401 和“本地监听凭证无效”；使用 listener token 文件返回 200 与 `{"models":[]}`。这在不修改任何状态的情况下实验确认了配置投影与 listener 凭据不一致的根因。
- 当前选中 `claude-openai-chat` 供应商的脱敏配置显示其上游 `OPENAI_API_KEY` 长度为 51，不等于 Home 中的 13 字节值；因此错误值也不是当前供应商 API key。第一次枚举字段哈希时脚本变量名 `path` 覆盖了 zsh 特殊 PATH 数组，导致 `shasum/awk` 找不到；下一次使用 `json_path` 与绝对命令路径，避免重复失败。
- `switch_provider()` 只交换 runtime 的供应商快照并更新 DB route/failover，不重建或重写 Home 配置。启用时遗留的错误 listener token 不会因后续供应商切换自愈；这解释了 UI 显示正确供应商但所有请求持续在 listener 认证层失败。
- 安全哈希枚举确认 Home 的 13 字节 token 不匹配任何 Codex 供应商的上游 key。进一步与代码常量比较后精确确认它就是旧全局接管占位符 `PROXY_MANAGED`。
- 两个用户现象因此不仅同时存在，而且形成因果链：Codex 页面错误显示旧全局 `ProxyToggle` → 用户点击后旧全局接管把 `PROXY_MANAGED` 写入 Live Home → 随后 Profile 路由启用创建 43 字节私有 token，但配置构建器保留 `PROXY_MANAGED` → listener 对真实请求返回 401。
- `secret_store` 只有受迁移证明约束的兼容入口才允许用 `PROXY_MANAGED` 初始化 token；普通新 Profile 的 `ensure_token` 正确生成随机 token。问题不是兼容常量存在，而是新 Profile 配置投影没有覆盖旧占位符。
- 启动恢复流程 `restore_enabled_profiles()` 对已启用 Profile 只执行“读取供应商快照 → `ensure_token` → 启动 runtime → 健康检查”，不会重建或应用 Home 配置计划。因此当前已启用的 `codex-api` 即使重启 CC Switch，仍会继续使用错误的 `PROXY_MANAGED`，必须在启动恢复中加入配置对账/自愈。
- 数据库还保留旧应用级 `proxy_config` 的 `codex|enabled=1` 和 `proxy_live_backup` 记录。迁移服务只在“首次新建且应用旧状态”时把路由与 backup 转移到 Profile；对已存在 Profile 的日常启动不会清理这两条遗留状态。这些遗留状态可以继续驱动顶部旧开关显示为开启，不能再作为 Codex Profile UI 的状态源。
- 前端已有按 Profile ID 分区的 TanStack Query key：`["codexProfiles", "state", profileId]`，也已有 `enableRoute` / `disableRoute` / `switchProvider` 三个窄 API。因此 UI 修复不需要新的后端启停命令，只需将 Codex 顶部开关改为强制携带 `selectedProfileId` 的专用 mutation/组件。
- `App.tsx` 中 Provider 卡片点击已按当前 Profile 执行：路由开启时调 `switchProvider`，路由关闭时调 `enableRoute`。新顶部开关应与这套语义保持一致：开启使用当前 Profile 的 `currentProviderId`，关闭只停止当前 Profile，并只失效当前 Profile 的 state query。
- `CodexHomeContextBar` 已显示选中 Home、端口与路由开启状态；顶部开关可继续保留在原来的应用栏位置，但其 tooltip/可用性/加载态必须来自当前 Profile，而不得读取 `useProxyStatus().takeoverStatus.codex`。
- Home 备份不只保存路由前正文，还保存当前路由目标的指纹。对已启用 Profile 做 token 自愈时，不能只改 Home 文件：还必须保留最初的 `previous_content/previous_fingerprint`、将 `target_fingerprint` 更新为新配置，并对“文件已写但 DB 未写”的崩溃窗口设计可恢复补偿，否则之后关闭路由会因指纹不匹配而拒绝恢复。
- 新鲜启用流程的最小正确顺序是：获取 Profile token → 用同一 token 构建配置计划 → 写补偿记录 → 启动并健康检查 runtime → 持久化原始 Home 备份 → 原子应用 Home 计划 → 持久化 enabled route。这样 token 从一个真值源流向 listener 和 Home，不存在两次生成或保留占位符的窗口。
- 顶部 Codex Profile 开关的可测语义已明确：`checked = selectedProfile.route.enabled`；开启时必须同时存在 `selectedProfileId` 和该 Profile 的 `currentProviderId`；关闭时只传当前 ID；成功后只失效 `state(profileId)`；切换 Home 时不得短暂显示上一 Profile 的开关状态。
- 旧全局 Codex 状态不只影响 UI：当前 DB 中 `proxy_config(codex)` 仍是 `proxy_enabled=1, enabled=1, listen_port=15721`，且保留 `proxy_live_backup`。通用启动恢复会枚举 `claude/codex/gemini` 的 `enabled`，对 Codex 再调 `set_takeover_for_app("codex", true)`；所以即使替换了前端开关，若不退役旧 Codex 全局状态，每次启动仍可以重新写入 `PROXY_MANAGED`。
- 当前启动时序更放大了竞态：Profile runtime 恢复被单独 spawn；之后通用崩溃恢复可先根据全局 backup 改 Home，再根据 `proxy_config.enabled` 重开全局 Codex 接管。Profile 恢复又不对账 Home，导致“Profile listener 运行、Home 却持有全局占位 token”成为可重现的稳态。
- 推荐的启动边界需串行化：先幂等退役/关闭旧 Codex 全局接管，保证其 backup 已恢复或已被 Profile 安全接管；再恢复各 Profile runtime 并对账 Home；最后通用代理恢复只处理 Claude/Gemini。不能让旧全局流程与 Profile 流程并发争抢同一 Home。

## 技术决策

| 决策                                                                | 理由                                                                                                                       |
| ------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| 先分别追踪“UI 路由状态作用域”和“listener token 写入/校验”两条数据流 | 两个现象可能共享 Profile 选择边界，也可能是独立缺陷；分层取证可避免把症状混为一个修复                                      |
| 不把数据库 route 行误判为共享状态                                   | 当前两个 Profile 已持有独立 `current_provider_id/enabled`；截图 1 更可能来自旧的全局 UI 开关/供应商读取链路                |
| 采用独立的 Profile 路由开关，不继续复用应用级 `ProxyToggle`         | 两者的标识、状态源、启停 API 和补偿语义都不同；独立组件能将 Profile ID 作为强制输入，避免再次回落到全局状态                |
| 在 runtime 启动前先确保 token，并将同一 token 投影到 Home           | listener 和 Codex Home 必须共享同一个认证真值；将 token 作为配置计划的必填参数，从类型边界消除漏写                         |
| 启动恢复时执行 Home 配置对账与自愈                                  | 修复不能只覆盖下一次手动重启路由；现有已启用的 `codex-api` 应在新版启动后直接恢复可用                                      |
| 将旧 Codex 全局接管从启动恢复中退役，保留 Claude/Gemini 旧逻辑      | Codex 已经以 Profile 为最高作用域；两套生命周期同时写 Codex Home 必然产生串状态，而其他应用不应受此修复影响                |
| 旧 Codex 退役时区分“已有启用 Profile”与“无启用 Profile”             | 前者不能盲目用全局 backup 覆盖 Profile Home，应清理旧 DB 状态后由 Profile 对账；后者仍须走旧安全恢复路径，避免留下占位配置 |
| 对账 recovery 只保存旧/新指纹，不保存 token                         | 指纹足以在崩溃后区分“未写”、“已写未提交”和“外部修改”，同时不扩大凭证泄漏面                                                 |

### 方案比较结果

1. **采用：Profile 专用开关 + token 单真值源 + 旧全局退役 + 启动自愈。** 能同时修复 UI 作用域、当前 401 和重启回归，并从类型/API 边界防止再次串用。
2. **不采用：给通用 `ProxyToggle` 增加 Codex Profile 分支。** 改动看似较小，但组件会同时承担应用级和 Profile 级两套状态/API，继续保留串用风险。
3. **不采用：只换前端开关，要求用户手动关闭再开启。** 无法清理旧启动恢复，当前 Profile 不能直接自愈，重启后还可能再次回写 `PROXY_MANAGED`。

已将采用方案写入：

- `docs/superpowers/specs/2026-07-14-codex-profile-route-isolation-auth-design.md`
- `docs/superpowers/plans/2026-07-14-codex-profile-route-isolation-auth.md`
- 文档自审已通过：无 TODO/TBD/待确认占位，采用与不采用方案无矛盾，前端 props、token 数据流、启动时序、补偿指纹与真实验收均已覆盖；三份文档 Prettier 和 `git diff --check` 通过。

## 遇到的问题

| 问题                                     | 处理方式                                                                            |
| ---------------------------------------- | ----------------------------------------------------------------------------------- |
| 真实配置 token 与 listener token 不一致  | 继续反向追踪 Home 配置计划从何处取得 bearer token，以及后续写入是否被旧全局同步覆盖 |
| UI 告警显示默认 Profile 的官方供应商     | 继续追踪顶部路由开关/告警使用的 `currentProviderId` 与选中 Profile 状态来源         |
| 哈希枚举脚本覆盖 zsh `path` 特殊变量     | 改用非保留变量名和绝对命令路径，原命令不再重复                                      |
| 三份新 Markdown 首次 Prettier 检查未通过 | 不重复相同检查，改用 `prettier --write` 格式化三份文档，之后再执行只读复核          |

## 已确认根因

1. **Codex 顶部路由开关接错生命周期。** `App.tsx` 对 Codex 仍渲染应用级 `ProxyToggle`；该组件调用没有 Profile ID 的 `set_proxy_takeover_for_app("codex")`，读写默认/全局 Codex Live 配置和全局当前供应商。它与选中 Profile 的 `codexProfileState`、独立端口和 `CodexProfileRouteManager` 无关，所以在 `codex-api` 页面点击仍会读取默认 Profile 的 OpenAI Official 并产生截图 1 告警。
2. **Profile Home 配置计划漏写私有 listener token。** `enable()` 先构建计划、后 `ensure_token`；配置构建器根本没有 token 参数，只改地址、wire API 和模型。旧全局开关留下的 `PROXY_MANAGED` 因此被保留，而 listener 使用新生成的 43 字节随机 token，所有 Codex 请求在本地认证 middleware 被 401 拒绝。
3. **测试覆盖缺口让两个问题同时漏过。** UI 没有断言 Codex 顶部开关按 selected Profile 调用 enable/disable；Home 配置测试没有断言计划写入精确 listener token，也没有覆盖旧 `PROXY_MANAGED` 被新 Profile token 替换。
4. **启动恢复缺少配置对账，让已启用的错误状态永久化。** `restore_enabled_profiles()` 只恢复 runtime，不校准 Home 配置；所以重启进程不会修复 401。同时旧应用级 Codex `enabled/live_backup` 仍存在，使用旧全局开关时会继续暴露过期状态。

## 实施与真实验收结果

- Codex 顶部开关已替换为 Profile 专用开关；状态、启停命令、供应商引用和查询失效范围都强制绑定当前选中的 Profile。默认 Profile 与自定义 Profile 权限完全相同，任何 Profile 都可以选择官方订阅或独立路由。
- Profile 启用流程已改为先取得唯一 listener token，再将同一 token 同时传给 runtime 和 Home 配置计划；关闭、重复关闭与失败补偿采用幂等状态边界。启动时先退役旧全局 Codex 接管，再按 Profile 串行对账 Home、备份指纹、恢复 listener；Claude/Gemini 旧逻辑保持不变。
- 启动对账覆盖配置正确、旧 `PROXY_MANAGED`、配置陈旧、外部修改、写入后崩溃和恢复失败边界。现场遗留的 pending disable 最终安全恢复原 Home，旧全局 Codex `enabled/live_backup` 被清理且重启后没有再出现。
- Profile listener 的请求日志和 body dump 已绑定 `profile_id`。真实 `/v1/responses` 请求返回 200 后，数据库最新请求行归属 `b341359e-437d-452c-a7ee-1019d99ae939`，body dump 写入同一 Profile ID 目录，不再依赖端口或当前选中 Home 反推归属。
- `cargo test` 曾因 NVM 目录下名为 `cc` 的 Node 脚本遮蔽系统 C 编译器而不能稳定重建；项目 `.cargo/config.toml` 现显式使用 `/usr/bin/cc`，避免 shell PATH 改变 Rust 链接器。

### 真实 UI 与请求证据

- 使用当前 Rust debug 二进制和 `dev:renderer` 启动现有 debug App bundle 后，Computer Use 可操作真实 Tauri UI。默认 Home 显示 `/Users/qihoo/.codex`、端口 15721、路由关闭；切到 `codex-api` 显示 `/Users/qihoo/.codex-api`、端口 15722、路由关闭，两个开关状态互不串用。
- 在 `codex-api` 页面开启路由后仅该 Profile 变为“端口 15722 · 路由已启用”；切回默认 Profile 仍为关闭，且没有再出现读取默认 OpenAI Official 的旧全局告警。
- Home `config.toml` 与 Profile listener token 均为 43 字节，SHA-256 完全相同；使用 Home token 调用 `/v1/models` 返回 200。
- 真实 `/v1/responses` 请求经 15722 返回 200，Home 配置文件 mtime 在请求及进程重启前后保持不变；重启后 listener 自动恢复，默认 Profile 仍关闭，自定义 Profile 仍开启，两个 Profile 均无 recovery/error 状态。
- 验收过程中临时把 `silentStartup` 改为 `false` 以观察窗口，结束后已恢复为 `true`；所有临时 dev/renderer/listener 进程均已停止，端口 13000、15721、15722、15723 无残留监听。

### 最终自动化验证

- `cargo test --manifest-path src-tauri/Cargo.toml --lib --quiet`：1844 通过、0 失败、2 忽略。
- `pnpm run test:unit` 首次全量验证：70 个测试文件、434 个测试全部通过；提交后并发复跑出现一次 1 文件/1 用例失败，但封装脚本只保留汇总，不能确认具体失败项。按失败协议改用 `pnpm exec vitest run --minWorkers=1 --maxWorkers=1 --no-file-parallelism` 后，70 个测试文件、434 个测试全部通过；未为验证修改产品代码、测试时限或 Vitest 配置。
- `pnpm run typecheck`、`cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`：通过。
- 首次误用 `pnpm test -- --run` 被 shell 解析为系统 `test`，未执行任何测试；随后读取 `package.json` 后改用项目实际脚本 `pnpm run test:unit`，没有重复失败命令。

### 完成审计续验

- 2026-07-14 续验时，工作区起点干净，实施计划没有未勾选步骤；当前 `pnpm run dev:dump` 进程仍在运行，Vite 监听 `[::1]:13000`，仅 `codex-api` Profile listener 监听 `127.0.0.1:15722`，默认 Profile 的 15721 没有监听。
- 当前实现入口仍明确按 Profile 分区：`App.tsx` 向 `CodexProfileRouteToggle` 传入 `selectedCodexProfileId`，启停 API 强制携带 Profile ID；后端启动顺序先调用旧 Codex 接管退役，再恢复并对账已启用 Profile，listener runtime、请求日志和 body dump 均持有 `codex_profile_scope`。
- Computer Use 直接按 “CC Switch” 连接当前 `tauri dev` 裸 Mach-O 进程仍会超时；应用枚举能看到两个 `com.ccswitch.desktop` 注册项，但都标记为未运行。该结果复现了此前已确认的 macOS 辅助功能注册边界，不是 UI 运行失败；续验改用现有 debug App bundle 承载当前 debug 二进制，同时保持 `pnpm run dev:dump` 后端作为权威运行态，不重复超时调用。
- 将当前 debug 二进制复制到现有忽略的 debug App bundle 后直接 `open`，单实例机制仍把启动转发给裸 `tauri dev` 进程；按 bundle ID 连接还会因为 `/Applications/CC Switch.app` 与 debug bundle 同 ID 而歧义，改用 debug bundle 绝对路径仍超时。因此 UI 续验必须先正常停止裸进程，再由同一 debug bundle 启动当前二进制；验收后重新恢复 `pnpm run dev:dump`，不能把 Computer Use 的附着限制误判为产品缺陷。
- 停止裸进程后，由同一当前 debug 二进制和 Vite renderer 启动 debug App bundle，Computer Use 成功附着。初始 `codex-api` 页面显示“切换 codex-api 路由”为开启、Home `/Users/qihoo/.codex-api`、端口 15722、路由已启用且 `claude-openai-chat` 使用中；切换到默认 Profile 后，开关立即变为“切换 默认 Codex 路由”且关闭，Home `/Users/qihoo/.codex`、端口 15721、路由未启用且 OpenAI Official 使用中。两个页面的状态和供应商引用没有串用。
- 从默认 Profile 再切回 `codex-api` 后，页面恢复该 Profile 自己的开启状态、15722 端口和 `claude-openai-chat` 使用中状态；没有短暂或持久继承默认 Profile 的关闭状态和官方供应商引用。
- 续验记录关闭前安全快照：默认 Home 配置 SHA-256 为 `15afa106...a764`，自定义 Home 配置 SHA-256 为 `98b84532...8350`，数据库为默认关闭、自定义开启且两者均无 error/recovery。通过 `codex-api` 专用开关关闭后，UI 只把该 Profile 改为“路由未启用”，仍保留端口 15722 和自己的供应商引用，没有出现旧的全局官方供应商告警。
- 关闭后的后端证据与 UI 一致：两个 Profile route 均为关闭且无 error/recovery，15721/15722 均无 listener；默认 Home 配置 SHA-256 仍是 `15afa106...a764`，证明操作没有改动默认订阅配置。随后再次点击 `codex-api` 专用开关，UI 仅将该 Profile 恢复为开启和端口 15722，供应商仍为 `claude-openai-chat`，全程未出现旧全局告警。
- 重新开启后的 Home token 与 listener token 均为 43 字节，SHA-256 同为 `e66ed8cd...535b`；数据库为默认关闭、自定义开启且无 error/recovery，只有 15722 listener 存在。默认 Home 配置 SHA-256 第三次核对仍为 `15afa106...a764`，证明自定义 Profile 完成“关 → 开”生命周期后默认订阅配置字节级未变。
- `codex-api` 完成“关 → 开”后再次切到默认 Profile，Computer Use 仍显示“切换 默认 Codex 路由”为关闭、端口 15721、路由未启用且 OpenAI Official 使用中；这给出了同一次真实 UI 生命周期内“自定义路由切换不改变默认 Profile”的直接证据。
- UI 续验结束前已把当前选择恢复为 `codex-api`，页面最终状态是专用开关开启、端口 15722、路由已启用、`claude-openai-chat` 使用中。真实请求前的基线为：自定义 Home 配置 SHA-256 `98b84532...8350`、mtime `1784028774`、该 Profile 请求日志 0 条、Profile body dump 目录已有 2 个历史文件。
- 当前 debug 二进制的 body dump 是编译期开关，且该二进制由本轮 `CC_SWITCH_DUMP_BODY=1 pnpm tauri dev` 构建，因此由 debug bundle 运行时仍保持 dump 开启。第一次真实请求测试脚本因 Node 22 同时检测到 CommonJS `require()` 和顶层 `await`，以 `ERR_AMBIGUOUS_MODULE_SYNTAX` 在客户端本地解析阶段退出，请求没有发出；后续改为 CommonJS `async` IIFE，不重复原脚本，也不修改产品实现。
- 修正后的真实 `POST http://127.0.0.1:15722/v1/responses` 返回 200，响应对象为 `response`、模型为 `Auto`、输出为精确的 `OK`，本地认证不再出现 401。数据库新增请求 `53c2bdb6-9c2d-4d3d-958f-56e11977dccf`，`profile_id` 精确归属 `b341359e-437d-452c-a7ee-1019d99ae939`、供应商为该 Profile 的 `4847cf65-...`、状态 200；Profile body dump 文件数从 2 增至 3，最新文件位于同一 Profile ID 目录。
- 请求前后自定义 Home 配置 SHA-256 均为 `98b84532...8350`、mtime 均为 `1784028774`；默认 Home 配置 SHA-256 仍为 `15afa106...a764`，旧 Codex 全局 backup 数量仍为 0。真实请求既没有重写自定义 Home，也没有触碰默认订阅配置或重新激活旧全局接管。
- 真实验收产生的唯一请求行和唯一新增 body dump 已按精确 ID/路径清理，Profile 请求日志恢复 0 条、body dump 目录恢复原有 2 个文件，未删除其他历史数据。临时 debug bundle/renderer 随后正常停止，最终再次运行 `pnpm run dev:dump`；当前二进制 1.45 秒完成构建并启动，日志明确显示只恢复 `codex-api` Profile listener 到 `127.0.0.1:15722`，主窗口按用户原设置静默隐藏。
- 最终 `pnpm run dev:dump` 进程上，Home token 与 listener token 再次确认均为 43 字节且哈希相同，使用 Home token 调用 `/v1/models` 返回 200 和 `{"models":[]}`。自定义 Home 配置 SHA-256/mtime 仍为 `98b84532...8350`/`1784028774`，默认 Home 配置 SHA-256 仍为 `15afa106...a764`；数据库仍是默认关闭、自定义开启且均无 error/recovery，旧 backup 为 0，实际监听仅有 Vite 13000 与 Profile 15722。
- 保持最终 dev 服务运行时重新执行完成前全量测试：Rust library 共 1846 项，1844 通过、0 失败、2 忽略；前端使用单 worker 稳定调度，共 70 个测试文件、434 个测试全部通过。测试输出中的 MSW/React/Tauri stderr 均来自现有错误分支或测试环境告警，命令最终退出码均为 0。
- 完成审计的 TypeScript 类型检查、Rustfmt、三份设计文档及受影响前端文件的 Prettier、`git diff --check` 均以退出码 0 通过。最终服务进程仍是 `pnpm run dev:dump`，监听保持为 13000/15722，没有 15721/15723 或临时 debug bundle 进程残留。
- 第一次提交后综合审计把多个“应监听/不应监听”端口合并给一个 `lsof`，业务值全部正确但命令因未监听端口返回 1；改为逐端口正反断言后，最终审计以退出码 0 通过：工作区干净、计划无未完成步骤、15722 正在监听、15721/15723 未监听、当前选择为 `codex-api`、token 哈希一致、`/v1/models` 为 200，数据库组合状态精确为 `默认关闭|自定义开启|0 错误恢复|0 旧 backup|0 验收请求残留`。

## 资源

- 截图 1：`/Users/qihoo/.codex/attachments/4a07bc73-22f0-4615-bdc5-8a8120affecb/image-1.png`
- 截图 2：`/Users/qihoo/.codex/attachments/4a07bc73-22f0-4615-bdc5-8a8120affecb/image-2.png`

## 视觉 / 浏览器发现

- 截图 1 是黄色告警条，完整文案为：“当前供应商 OpenAI Official 是官方供应商，建议切换到第三方供应商后再使用本地路由”。该告警描述的是“当前供应商”而不是当前 Profile 的已选路由供应商，存在读取全局 Codex 当前供应商的明显嫌疑。
- 截图 2 显示当前 CODEX_HOME 已明确选中 `codex-api · /Users/qihoo/.codex-api`，端口 `15722`，状态“路由已启用”；供应商列表中 `claude-openai-chat` 卡片呈绿色选中态。说明 UI 的 Profile 上下文栏和卡片选中态已经读取到自定义 Profile，但顶部全局路由开关仍可能沿用旧的全局 Codex 开关逻辑。
- 截图 2 中 `OpenAI Official` 标为“不支持路由”，多个第三方供应商标为“需要路由”；当前被选中的 `claude-openai-chat` 指向 `http://127.0.0.1:7072/v1`。

---

_每执行 2 次查看、浏览器或搜索操作后更新此文件。_
