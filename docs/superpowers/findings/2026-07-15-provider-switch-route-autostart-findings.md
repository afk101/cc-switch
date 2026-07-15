# 供应商切换自动开启路由 Findings

## 需求

- 初始路由关闭时，切换供应商配置不得自动开启路由。
- 官方订阅配置不需要路由，切换后必须保持路由关闭。
- 切换到需要路由的供应商时，只提示用户需要手动开启路由，不代替用户开启。
- 恢复此前正确的交互语义，不改变用户显式点击路由开关的行为。

## 研究发现

- `src/App.tsx:383-400` 的 Codex 专用切换分支在路由关闭时直接调用 `codexProfilesApi.enableRoute(...)`；这不是后端隐式行为，而是前端明确要求启用路由。
- 该逻辑由提交 `0e2fd337 feat(codex): add home profile context UI` 引入。此前 Codex 与其他应用相同，统一调用 `useProviderActions.switchProvider`。
- 原有 `useProviderActions.switchProvider` 会识别需要代理的供应商并只显示 `proxyRequiredForSwitch` 警告，然后仍执行普通供应商切换；它不会启动代理。
- 路由已启用时调用 `codexProfilesApi.switchProvider` 是正确的热切换语义；错误仅在路由关闭分支把“切换配置”错误等同于“启用 Profile 路由”。
- 托盘切换代码已有明确注释：需要本地路由的供应商也不自动启动代理，证明“切换不改变路由开关”是项目既有不变量。
- 当前缺少覆盖 `App.handleSwitchProvider` 路由关闭分支的前端测试，因此回归未被测试阻止。
- 原多 Home 设计明确要求任意 Profile 可使用官方订阅或第三方直连，并要求所有 Codex mutation 携带 `profile_id`；因此不能简单退回全局 `providersApi.switch`。
- 旧计划只定义了一个 Profile `switchProvider` API，却没有区分“运行中热切换”和“停止时直接应用配置”，接口语义不完整，促使前端用 `enableRoute` 填补缺口。
- `ProviderService::switch_normal` 同时承担全局 current provider、默认 Codex Home live 写入、回填与 MCP 同步；直接复用会错误写入默认 `~/.codex`。
- `codex_config::write_codex_live_atomic_for_home(home, content)` 已支持显式 Home 的 config-only 原子写入；现有 `prepare_codex_provider_live_config` 与官方统一会话 helper 可复用来构造目标配置。
- Profile 直连切换需要明确处理官方 provider 与第三方 provider：官方可写其配置并保留该 Home 自己的 OAuth；第三方在保留官方登录设置启用时只写 config，不复制或覆盖 `auth.json`。
- `tests/integration/App.test.tsx` 当前把“点击 Codex 供应商后路由已启用”写成成功断言；该测试固化了错误行为，需要反转为“路由仍未启用且供应商已切换”。
- 现有 route toggle 单元测试只覆盖用户显式点击开关时调用 `enableRoute`，这正好可作为不变量：只有路由开关拥有启用权限。
- `CodexRouteManager::switch_provider` 当前无 `route.enabled` 分支，直接要求现存 runtime 并执行快照热切换；路由关闭时无法使用。
- `CodexHomeConfigService` 已封装显式 Home 的原子读写和指纹校验，且 route plan 已接收 `Provider`；在该边界新增“构造并应用直连 provider config”比让 App 或命令层直接写文件更符合职责分离。
- 关闭态切换必须先成功写入目标 Home，再持久化 `current_provider_id`，写入失败时保持旧选择；不得生成 listener token、启动 runtime、创建 live backup 或把 `enabled` 改为 true。
- 模型目录生成目前硬编码到默认 Codex Home；关闭态 Profile 直连切换若携带 `modelCatalog`，必须增加显式 Home 版本，确保 `cc-switch-model-catalog.json` 写入选中的 Home 而不是 `~/.codex`。
- MSW 将 `enable_codex_profile_route` 与 `switch_codex_profile_provider` 共用同一个 `enabled=true` handler，也同步固化了错误；应拆成“enable 设置 true”和“switch 只保留当前 enabled”两个 handler。
- `setCodexProfileRoute(profileId, providerId, enabled)` 已能表达测试状态，MSW 的 switch handler 可读取现有状态后仅更新 provider，或新增单一职责 helper。
- 推荐把 `switch_codex_profile_provider` 命令语义扩展为“在当前 Profile 状态下选择供应商”：启用时热切换，关闭时直连应用；这样 App 不再读取可能陈旧的 route 快照决定生命周期操作。
- 运行 `pnpm exec vitest run tests/integration/App.test.tsx -t "covers basic provider flows via real hooks"` 通过，且用例明确等待“路由已启用”，稳定证明当前实现会在初始关闭状态下自动开启路由。

## 根本原因

- 前端 `handleSwitchProvider` 将两个正交状态错误绑定：当 `route.enabled = false` 时，它调用生命周期命令 `enableRoute`，导致任何供应商选择都被解释成用户授权开启路由。
- Profile API 只提供“启用路由”和“运行中热切换”，缺少“路由关闭时对指定 Home 应用普通供应商配置”的能力；前端用启路由绕过了这个接口缺口。
- 集成测试断言了错误行为，未建立“普通选择绝不能改变 enabled”的回归不变量。

## 方案比较

### 方案 A（推荐）：统一 Profile 供应商选择命令，后端按权威状态分派

- 前端始终调用 `switch_codex_profile_provider`，不再调用 `enableRoute`。
- 后端在 Profile 锁内读取 `route.enabled`：启用则走现有 runtime 热切换；关闭则对显式 Home 应用直连配置并保持 `enabled=false`。
- 优点：生命周期权限集中、避免前端快照竞态、所有调用入口行为一致。
- 代价：需要补充显式 Home 的 provider config/catalog 事务与后端测试。

### 方案 B：前端根据状态调用两个不同命令

- 新增 `apply_codex_profile_provider_direct`，前端继续判断 route 状态。
- 优点：改动直观。
- 缺点：状态可能在点击与命令执行间变化，未来托盘/深链调用者也容易再次选择错误命令。

### 方案 C：路由关闭时退回全局 `switch_provider`

- 优点：复用旧警告和普通切换流程。
- 缺点：该命令写默认 Codex Home 和全局 current provider，会破坏选中 Home 的隔离，违反多 Profile 设计。

## 推荐设计摘要

- 采用方案 A。
- `enable_codex_profile_route` 是唯一能把 `enabled` 从 false 改成 true 的命令。
- `switch_codex_profile_provider` 只选择供应商：关闭态直连写入，启用态热切换，绝不隐式启路由。
- 关闭态直连写入只触碰目标 Home 的 `config.toml` 和其 CC Switch 自有模型目录文件，不复制或覆盖 `auth.json`。
- 写 Home 成功但数据库失败时恢复原 Home；Home 写入失败时不更新数据库。
- 前端在路由关闭且供应商需要本地路由时保留原有 warning，由用户显式点击开关。

## 技术决策

| 决策                                                              | 理由                                                                                  |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| 将供应商选择与路由生命周期彻底分离                                | 用户选择供应商不等于授权启动本地服务或改写 Home 为本地地址。                          |
| 路由启用时保留 Profile 热切换                                     | 运行中的 Profile 必须原子更新运行时快照与数据库状态。                                 |
| 路由关闭时只执行目标 Home 的普通配置切换                          | 官方订阅与可直连供应商无需路由；需要路由的供应商沿用警告、由用户手动开启。            |
| Profile API 内部根据 route.enabled 分派热切换或直连切换           | 前端只表达“选择此供应商”，后端在同一 Profile 锁内依据权威状态执行，避免 UI 快照竞态。 |
| 只有 `enable_codex_profile_route` 能把 enabled 从 false 改为 true | 用后端命令边界保证任何 UI、托盘或未来调用者都无法通过普通供应商选择隐式启动路由。     |

## 遇到的问题

| 问题                                             | 解决方案                                                                                |
| ------------------------------------------------ | --------------------------------------------------------------------------------------- |
| 当前切换动作错误地自动启路由                     | 删除 `enableRoute` 分支，替换为显式 Home 的普通供应商应用接口。                         |
| 普通 `switch_provider` 默认作用于默认 CODEX_HOME | 调查并复用 ProviderService 配置构造逻辑，新增 Profile Home 作用域而不是临时改环境变量。 |

## 资源

- `src/components/codex/`
- `src-tauri/src/codex_profile/`
- `src/App.tsx:383-400`
- `src/hooks/useProviderActions.ts:152-277`
- `src-tauri/src/tray.rs:461-463`

## 视觉 / 浏览器发现

- 本问题是行为与状态流问题，不需要视觉方案比较。

---

_每执行 2 次查看/浏览器/搜索操作后更新此文件_
