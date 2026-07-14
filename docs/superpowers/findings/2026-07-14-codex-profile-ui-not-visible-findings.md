# Codex Profile 新功能未显示 Findings

## 用户第一手现象

- 用户通过 `pnpm run dev:dump` 启动服务后，在 Codex 页面看不到多 `CODEX_HOME` 的新功能。
- 用户提供的截图显示：顶部已选择 Codex，但主区域仍是旧版全局供应商列表；页面没有 Home/Profile 选择器、Profile 管理入口或端口/运行状态信息。

## 调查范围

- 先确认该现象的复现条件、当前运行的工作树与前端加载路径。
- 在根因确认前不修改产品代码、不猜测 UI 实现状态。

## 用户补充

- 用户询问 `pnpm run dev:dump` 是否应具备热更新能力，尚未确认是否曾手动重启开发进程。
- 调查将以项目脚本的真实行为为准，区分前端热更新与 Rust 后端重编译/重启。

## 已收集证据

- `package.json` 中 `dev:dump` 为 `CC_SWITCH_DUMP_BODY=1 pnpm tauri dev`；它不是单独的前端预览命令。
- 工作树最近已提交 Profile 数据库、迁移、Home 配置、私有 token 和独立路由运行时的 Rust 实现。
- 在 `src/` 中没有 `CodexHomeContextBar`、`CodexProfileManagerDialog`、`codexProfile` 前端 API/查询/类型文件，也没有 `list_codex_profiles` 或 `selected_codex_profile_id` 的前端调用。
- 因此截图中的旧供应商列表与当前前端源码一致，不能先归因于 Tauri 未重启或 Vite 热更新失效。

## 前端调用链对照

- `src/App.tsx` 以 `activeApp === 'codex'` 作为现有全局供应商界面的应用选择条件，并将该值传给 `useProvidersQuery` 和 `ProviderList`。
- 该组件没有导入或渲染 Profile 上下文栏/管理弹窗，也没有 Profile 查询、Profile 选择状态或按 Profile 维度的 mutation。
- 已提交的 `1c5d0af4` 仅新增 Rust `RouteRuntime` 和代理层隔离能力；它没有新增 `src/types/codexProfile.ts`、`src/lib/api/codexProfiles.ts`、`src/lib/query/codexProfiles.ts` 或任何 UI 组件。

## 根本原因

- `pnpm run dev:dump` 的开发模式与热更新并非根因。截图显示的是当前已经实现的旧前端。
- 多 `CODEX_HOME` 实施只完成了后端基础与运行时阶段；负责把功能暴露到桌面界面的前端类型/API/缓存层和 Home 上下文栏/Profile 管理 UI 尚未实现、也未接入 `App.tsx`。
- 因此前端没有任何可渲染的新功能入口，重启或刷新都只能继续显示全局供应商列表。

## Brainstorming 决定

- 用户拒绝使用浏览器可视化伴侣，本次 UI 方案通过纯文本协作确认。
- 用户确认：Codex 页在供应商列表上方提供固定的当前 Home 上下文栏，显示 Home 名称、路径、端口、路由状态，并从此处切换或打开 Home 管理。
- 用户确认：供应商定义和列表全局共享；路由启用、当前供应商、故障转移、端口与运行状态严格由当前选中 Home 决定。
- 用户确认：最后选中的 Home 写入设置，启动时优先恢复；不存在时回退默认 `~/.codex` Profile。
- 用户纠正端口约束：默认 `~/.codex` Profile 也必须允许修改监听端口，不能因多 Home 改造而回归既有端口配置能力；默认 Profile 仅不可删除、不可重绑 Home 路径。

## 既有端口能力核对

- 现有 `src/components/proxy/ProxyPanel.tsx` 已允许编辑全局代理监听端口，默认值为 `15721`。
- Profile 数据模型也已有独立 `listen_port` 字段和唯一约束；前端方案应将此能力迁入当前 Home 的管理界面，而不是将默认 Profile 端口设为不可编辑。

## 已批准的设计决策

| 决策 | 理由 |
|---|---|
| 采用独立 Profile 上下文层 | 将当前 Home 作为 Codex 页最高作用域，避免继续复用全局代理状态而串实例。 |
| 供应商定义全局共享 | 同一供应商可由多个 Home 使用，不复制供应商配置。 |
| 路由状态按 Profile 隔离 | 当前供应商、故障转移、端口、启用状态必须显式携带 `profileId`。 |
| 持久化最后选中 Home | 重启后恢复用户正在管理的实例；失效才回退默认 Profile。 |
| 默认 Profile 可改端口 | 保持旧全局监听端口的编辑能力；仅限制默认 Profile 删除与 Home 重绑。 |
| 删除仅解绑 | 删除自定义 Profile 时只删除 CC Switch 关系与本地 token，永不删除 Home 目录。 |

## 方案比较与结论

- 推荐完整 Profile 上下文层：先暴露 Profile 命令，再建立前端类型/API/查询缓存，最后渲染上下文栏与管理弹窗。
- 未采用仅前端展示：不能实际管理或隔离路由状态。
- 未采用复用旧全局代理接口：会混淆全局代理和 Profile 路由，无法保证切换后的状态隔离。
