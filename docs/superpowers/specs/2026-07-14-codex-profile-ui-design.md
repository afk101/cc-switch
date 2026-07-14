# Codex Profile UI Design

## 目标

让已存在的多 `CODEX_HOME` 后端能力可在桌面端可见、可选择、可管理，并保证一次 Codex 操作只作用于当前选中 Profile。

## 范围

- Codex 页供应商列表上方新增固定 Home 上下文栏。
- 新增 Profile 管理弹窗，用于创建、重命名、重绑自定义 Home、修改端口及删除自定义 Profile。
- 暴露 Profile 感知的 Tauri 命令、前端 DTO、API 和 TanStack Query 缓存。
- 供应商定义仍全局共享；路由状态和操作严格按 `profileId` 隔离。

不在本切片中复制供应商、删除任何 Home 文件，或改变 Claude/Gemini/OpenCode 等应用的单例逻辑。

## 交互与状态

Codex 页面加载 Profile 列表后，从 `selected_codex_profile_id` 恢复选择；该 ID 不存在时选择默认 `codex-default`。上下文栏显示当前 Home 名称、规范化路径、监听端口、路由状态、当前供应商与最后错误。切换 Home 后，旧 Profile 的路由状态不得作为新 Profile 的回退内容显示。

供应商列表仍展示全局 Codex 供应商。对当前 Provider 的切换、启用/停用路由和故障转移操作必须调用 Profile 专用 API，参数中始终包含当前 `profileId`。

## Profile 管理规则

| 项目 | 默认 Profile | 自定义 Profile |
|---|---|---|
| 名称 | 可修改 | 可修改 |
| Home 路径 | 不可重绑 | 可重绑 |
| 监听端口 | 可修改 | 可修改 |
| 删除 | 不可删除 | 允许删除 |

新建自定义 Profile 自动分配可用端口，也可在提交前填写手动端口。端口重复、已被监听或 Home 路径重复时，显示后端的可定位错误。删除确认必须明确说明“只删除 CC Switch 绑定和本地 token，不删除 Home 目录、auth.json、config.toml 或会话”。

## 后端边界

命令层只校验 DTO、加载当前 Profile 并委托 repository 或 route manager；不得在命令中直接写 SQL、绑定端口或修改 Home 文件。每个状态变更发出 `codex-profile-state-changed`，payload 包含 `profileId`、状态、供应商 ID 和端口。

此 UI 依赖独立 `RouteRuntime` 和 `CodexRouteManager` 完成启动、切换、停用、删除的事务与回滚；若运行时尚未可用，UI 显示明确错误，不得静默回退旧全局代理接口。

## 验收标准

- `pnpm run dev:dump` 的 Codex 页可见 Home 上下文栏和“管理 Home”入口。
- 选择 A 后只显示并操作 A 的路由状态；切换 B 时不显示 A 的当前供应商或错误。
- 默认 Profile 可以修改端口，但不能删除或重绑路径。
- 自定义 Profile 删除确认不承诺也不执行文件删除。
- 同一供应商可被 A、B 路由引用；切换 A 的供应商不改变 B。
- 前端测试覆盖查询键隔离、切换时无陈旧状态、管理规则和确认文案；Rust 命令测试覆盖每个 mutation 的 `profileId` 作用域。
