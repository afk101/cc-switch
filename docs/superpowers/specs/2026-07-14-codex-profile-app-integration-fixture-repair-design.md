# Codex Profile App Integration Fixture Repair Design

## 背景

Codex 页面已经以选中 Profile 的 `route.currentProviderId` 作为当前供应商真相源，但 App 集成测试仍只模拟旧的全局供应商查询。缺少 `list_codex_profiles` 和 `get_codex_profile_state` 响应时，测试把空供应商 ID 映射成 `undefined`，因此点击用量按钮后不会打开用量弹窗。

## 目标

- 让 App 集成测试按真实产品数据流加载默认 Codex Profile 及其独立路由状态。
- 覆盖 Codex 页面加载、用量弹窗、启用 Profile 路由和供应商切换的完整 Tauri 命令链。
- 保持多 `CODEX_HOME` 的 Profile 作用域，不在产品代码中回退到全局供应商状态。
- 完成前端与 Rust 全量测试，并使用 `pnpm run dev:dump` 和 Computer Use 重新验证真实 macOS UI 闭环。

## 非目标

- 不修改生产环境的 Profile 数据模型、路由实现或供应商选择语义。
- 不新增测试专用的产品分支或运行时回退。
- 不重构与 App 集成夹具无关的测试基础设施。

## 推荐设计

### 状态边界

`tests/msw/state.ts` 增加可重置的 Codex Profile 状态映射。默认状态包含：

- Profile ID：`codex-default`；
- Home：`/default/codex`；
- 监听端口：`15721`；
- 当前供应商：`codex-1`；
- 路由初始为停止且未启用。

状态模块负责创建、深拷贝、读取和更新测试状态；所有新增 TypeScript 函数使用 JSDoc，并保持单一职责。

### Tauri 命令边界

`tests/msw/handlers.ts` 增加以下共享 handler：

- `list_codex_profiles`：返回 Profile 列表；
- `get_codex_profile_state`：按 `profileId` 返回独立快照；
- `enable_codex_profile_route`：校验 Profile/供应商后启用路由并更新当前供应商；
- `switch_codex_profile_provider`：校验 Profile/供应商后更新当前供应商并保持路由启用。

未知 Profile 或未知供应商返回 404；成功命令返回 `true`。handler 不直接维护副本，只调用状态模块接口。

### 集成测试边界

`tests/integration/App.test.tsx` 在切换到 Codex 后等待两个独立前置条件：供应商列表包含 `codex-1`，且 `current-provider` 已稳定为 `codex-1`。随后断言用量弹窗接收到 `codex-1`，并在点击供应商切换后等待 Profile 上下文显示路由已启用。

## 数据流

1. App 查询 Profile 列表并选中 `codex-default`。
2. App 使用该 ID 查询 Profile 状态。
3. `ProviderList.currentProviderId` 从 Profile 路由取得 `codex-1`。
4. 用量操作把真实的 `codex-1` Provider 传给用量弹窗。
5. 供应商切换调用 Profile 路由命令；MSW 更新同一个 Profile 快照。
6. App 刷新 Profile 状态并显示路由已启用。

## 错误处理

- 状态读取找不到 Profile 时返回 `null`，由 handler 转为 404。
- 路由更新找不到 Profile 或供应商时返回 404，不静默成功。
- 集成测试等待真实交互条件，避免把异步查询未完成误判为产品失败。

## 验收标准

1. `tests/integration/App.test.tsx` 不再产生 Profile 命令的 MSW 未处理请求。
2. 用量弹窗明确显示 `codex-1`。
3. 点击 Codex 供应商切换后，Profile 上下文显示路由已启用。
4. 前端全量测试、类型检查和格式检查通过。
5. Rust 全量测试、编译和格式检查通过。
6. `pnpm run dev:dump` 可启动；Computer Use 验证默认与自定义 Profile 管理、选择和独立路由路径无回归。
7. 源码仍无 Profile 管理 `window.prompt`，工作区最终干净。
