# Codex Profile 供应商切换不得自动开启路由设计

## 背景

多 `CODEX_HOME` 支持引入后，Codex 页面把“供应商选择”与“路由启用”错误绑定：当当前 Profile 路由关闭时，点击任意供应商会调用 `enable_codex_profile_route`。这会让官方订阅也被改写成本地地址并启动路由，违反用户授权边界和此前交互语义。

## 目标

- 供应商选择永远不隐式开启路由。
- 官方订阅供应商在路由关闭时直接应用到当前 Profile，并保持路由关闭。
- 需要本地路由的供应商在路由关闭时仍完成选择，但只显示“需要开启路由”提示；是否开启由用户点击路由开关决定。
- 路由已启用时继续使用现有 Profile runtime 热切换，不重启监听器。
- 所有操作严格作用于当前选中的 `CODEX_HOME`，不回退到默认 `~/.codex`。

## 不变量

1. 只有 `enable_codex_profile_route` 可以把 `route.enabled` 从 `false` 改为 `true`。
2. `switch_codex_profile_provider` 不改变 `route.enabled`。
3. 路由关闭时选择供应商不得创建 listener token、启动 runtime 或创建路由 live backup。
4. Profile 直连切换不得复制、覆盖或删除该 Home 的 `auth.json`；每个 Home 保持自己的 ChatGPT OAuth。
5. Profile A 的切换不得写 Profile B 或默认 Home 的配置和模型目录。
6. Home 写入失败时数据库不变；数据库保存失败时恢复切换前 Home 内容。

## 推荐架构

### 前端

`App.handleSwitchProvider` 对 Codex 始终调用：

```ts
codexProfilesApi.switchProvider(profileId, provider.id, []);
```

前端不再根据查询缓存中的 `route.enabled` 选择生命周期命令。查询缓存只用于展示状态和决定是否显示“需要手动开启路由”提示，不能授权后端启动服务。

新增纯函数 `getCodexProviderRouteRequirement(provider)`，统一识别 Codex 的 OpenAI Chat 格式和完整 URL 等需要路由的配置。路由关闭时命中该规则，复用现有 `notifications.proxyRequiredForSwitch` 文案显示 warning，但仍执行供应商选择。

### Tauri 命令层

`switch_codex_profile_provider` 从数据库读取目标 provider，并应用通用配置片段，得到 effective provider；随后只调用 route manager 的统一选择入口。命令层不读写 Home，不修改 enabled。

### Route manager

统一入口在取得当前 Profile 锁并完成 pending recovery 后读取权威 route：

- `enabled=true`：执行现有 runtime snapshot 热切换和可恢复持久化流程。
- `enabled=false`：构造显式 Home 的直连 provider plan，原子写入目标配置，然后保存 `current_provider_id`；整个过程保持 `enabled=false`。

关闭态分支不访问 runtime factory 和 secret store，不写 failover 运行策略；UI 当前传入空 failover，数据库清理旧 failover 引用。

### 显式 Home 配置计划

`CodexHomeConfigService` 新增直连 provider plan，内容包括：

- 切换前 `config.toml` 快照与指纹；
- 切换后的 `config.toml`；
- 可选的 `cc-switch-model-catalog.json` 目标内容及原快照；
- 唯一目标 Home 路径。

provider 配置生成规则：

- 官方 provider：应用官方统一会话配置规则，仅写 config，绝不写 provider 保存的 auth 到目标 Home。
- 第三方 provider：把 provider 保存的 API key 投影为活动 provider 的 `experimental_bearer_token`，仍不覆盖 Home 的 auth。
- 带 `modelCatalog` 的 provider：模型目录写到目标 Home，配置使用相对文件名。

应用计划时先写模型目录再写 config；任一步失败恢复已修改的文件。manager 后续数据库保存失败时调用 plan restore 恢复全部 Home 文件。

## 数据流

```text
点击供应商
  → 显示按需路由 warning（只提示）
  → switch_codex_profile_provider(profileId, providerId)
  → 获取 effective provider
  → Profile 锁 + 权威 route 状态
      ├─ enabled=true  → runtime 热切换 → 保存 provider 引用
      └─ enabled=false → 应用目标 Home 直连配置 → 保存 provider 引用
  → 刷新当前 Profile 状态
```

## 错误处理

- Profile/provider 不存在：返回明确错误，不改变文件或数据库。
- pending recovery 未收敛：沿用现有拒绝 mutation 机制。
- 官方 provider 在路由已启用时：沿用代理接管保护，拒绝热切换并提示先关闭路由。
- Home 指纹冲突：拒绝覆盖外部修改。
- Home 应用成功但数据库失败：恢复原 config/catalog；恢复失败进入现有补偿错误路径，不能谎报成功。
- 前端错误只显示 toast，不乐观修改 route 开关。

## 测试策略

### Rust

- 关闭态选择官方 provider：目标 Home 更新、auth 原样、enabled 仍 false、无 runtime/token/live backup。
- 关闭态选择第三方 provider：API key 投影到 config、auth 原样、enabled 仍 false。
- 带 model catalog 的 provider：目录只写当前 Profile Home。
- Home 写入失败：route/provider 引用不变。
- 数据库保存失败：Home config/catalog 回滚。
- 启用态供应商切换：现有热切换与补偿测试继续通过。
- 官方 provider 在启用态被拒绝。

### TypeScript / MSW

- Codex 点击供应商只调用 `switch_codex_profile_provider`，从不调用 `enable_codex_profile_route`。
- MSW switch handler 更新 provider 但保留原 enabled。
- 官方订阅切换后显示新 provider 且“路由未启用”。
- 需要路由的 provider 显示 warning，route 仍关闭。
- 用户显式点击路由开关仍调用 enable command。

## 验收标准

1. 初始路由关闭，切换官方订阅后仍显示“路由未启用”，并使用该 Home 自己的 OAuth。
2. 初始路由关闭，切换需要路由的第三方配置时只提示，157xx 监听器不会自动启动。
3. 用户随后手动打开路由，才启动该 Profile 的监听器。
4. 路由运行中切换第三方供应商仍为无重启热切换。
5. 不同 `CODEX_HOME` 的配置、auth、模型目录和路由状态互不串写。

## 非目标

- 不改变路由开关的布局或视觉样式。
- 不自动替用户判断并开启路由。
- 不迁移或共享不同 Home 的 ChatGPT 登录材料。
- 不修改非 Codex 应用的供应商切换语义。
