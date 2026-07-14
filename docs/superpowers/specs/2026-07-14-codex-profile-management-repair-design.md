# Codex Profile Management Repair Design

## 背景与根因

Codex Profile 管理弹窗的“新建 Profile”和“重新绑定 Home”使用 `window.prompt` 收集输入。macOS Tauri 2/WKWebView 不展示该 API 并直接返回 `null`，随后前端静默退出，所以用户稳定看到点击无响应。

现有实现还没有完成原 UI 设计中的重命名、创建时可选手动端口和删除确认；测试只覆盖默认 Profile 的部分按钮规则，而且前端把默认 Profile ID 错写为 `default`，与 Rust 常量 `codex-default` 不一致。

## 修复目标

- 在现有“管理 Codex Profile”Dialog 内完成创建、编辑和删除确认，不叠加第二个 Dialog。
- 完全移除 Codex Profile 管理链路对 `window.prompt` 的依赖。
- 创建时支持自动分配端口或手动指定端口，并保证 Profile 与空路由记录原子创建。
- 编辑时原子更新名称、Home 和端口，任何校验失败都不得产生部分更新。
- 默认与自定义 Profile 权限一致遵循既有领域规则：所有 Profile 可改名；默认 Profile 不可重绑 Home、不可删除；所有已停止 Profile 可改端口。
- 路由运行时只允许修改名称；Home 和端口在 UI 中禁用，后端继续强制校验。
- mutation、缓存刷新、Profile 选择和组件视图使用清晰、可独立测试的边界。
- 修复管理 UI 不得改变 Profile 级路由隔离；不同 `CODEX_HOME` 实例的请求正文、凭证、供应商快照和运行时状态绝不能跨 Profile 串流。

## 非目标

- 不改变多 `CODEX_HOME` 的路由生命周期协议。
- 不修改供应商全局共享与 Profile 路由引用模型。
- 不删除任何 Home 目录、`auth.json`、`config.toml` 或会话。
- 不移除现有 `rename_codex_profile`、`rebind_codex_profile`、`update_codex_profile_port` 命令；它们继续作为兼容接口保留。
- 不增加通用表单框架或新的 UI 依赖。

## 组件架构

### 管理工作流

`CodexProfileManagerDialog` 只协调一个 Dialog 内部的四种视图：

```ts
type ProfileManagerView =
  | { kind: "list" }
  | { kind: "create" }
  | { kind: "edit"; profileId: string }
  | { kind: "delete-confirm"; profileId: string };
```

Dialog 关闭时重置为 `list`。从创建、编辑或删除确认页取消时返回 `list`，不关闭 Dialog。关闭 Dialog 会丢弃未提交输入，不再叠加未保存确认。

### 受控表单

新增专用 `CodexProfileForm`，承担：

- 名称、Home、端口字符串状态；
- 创建与编辑初始值；
- 名称/Home 必填和去除首尾空格；
- 可选端口的整数及 `1–65535` 校验；
- 提交中禁用、防重复提交；
- 使用 `role="alert"` 就地展示前端或后端错误；
- 提交失败时保留全部输入。

表单通过 Promise 回调提交，不直接访问 Tauri API、TanStack Query 或 App 状态。

### Mutation 控制层

Profile 查询与 mutation 继续集中在 `src/lib/query/codexProfiles.ts`：

- 创建成功后刷新 Profile 列表；
- 更新成功后刷新列表及对应 `state(profileId)`；
- 删除成功后移除对应 state 缓存并刷新列表；
- 获取编辑目标状态继续使用 `codexProfileKeys.state(profileId)`，不得复用其他 Profile 状态。

专用管理 hook 组合 mutation 与 UI 选择规则，向 Dialog 提供 Promise actions。`App.tsx` 只负责打开管理器和维护当前选中的 Profile，不再内联创建、编辑、删除流程。

## 前端 DTO 与 API

新增明确输入类型：

```ts
interface CreateCodexProfileInput {
  name: string;
  homePath: string;
  listenPort?: number;
}

interface UpdateCodexProfileInput {
  profileId: string;
  name: string;
  homePath: string;
  listenPort: number;
}
```

`codexProfilesApi.create` 改为接收创建 DTO，并向 `create_codex_profile` 传递可选 `listenPort`。新增 `codexProfilesApi.update`，调用原子 `update_codex_profile` 命令。

前端常量文件定义：

```ts
export const CODEX_DEFAULT_PROFILE_ID = "codex-default";
export const CODEX_PROFILE_MIN_PORT = 1;
export const CODEX_PROFILE_MAX_PORT = 65_535;
```

Profile 管理代码不得继续使用 `"default"` 等魔法字符串判断默认 Profile。

## 后端领域边界

### 创建

`create_codex_profile` 新增可选 `listenPort: Option<u16>`。Repository 创建流程：

1. 校验并规范化名称；
2. 规范化 Home 并检查重复；
3. 端口为空时调用既有自动分配；
4. 端口存在时检查非零、其他 Profile 未保留且 `127.0.0.1` 可绑定；
5. 复用 `create_codex_profile_with_empty_route`，在同一 SQLite transaction 写入 Profile 与空路由。

手动端口校验和自动分配必须复用共同的端口可用性逻辑。

### 原子编辑

新增异步 Tauri 命令 `update_codex_profile`，一次接收 Profile ID、名称、Home 与端口。命令读取运行时状态后委托 Repository：

1. 加载原 Profile；
2. 校验新名称；
3. 规范化 Home，确定 Home 是否变化；
4. 确定端口是否变化；
5. 若运行中且 Home 或端口变化，拒绝整个更新；
6. 若默认 Profile 的 Home 变化，拒绝整个更新；
7. 若端口变化，检查数据库占用和系统监听占用；
8. 所有校验通过后，一次调用 DAO 更新名称、Home、端口和时间戳。

任一步失败时，数据库中的三个字段全部保持原值。运行中仅修改名称允许成功。

## 界面与交互

### 列表页

- 卡片显示名称、规范化 Home 和监听端口。
- 使用统一“编辑 Profile”入口，不再单独调用 prompt 重绑。
- 默认 Profile 的删除按钮禁用；自定义 Profile 可进入删除确认页。
- Footer 提供“新建 Profile”和“关闭”。

### 创建页

- 字段：Profile 名称、CODEX_HOME 路径、可选监听端口。
- 端口提示“留空则自动分配”。
- 名称输入自动聚焦。
- 创建成功后自动选中新 Profile、刷新列表、返回列表页，Dialog 保持打开并显示成功提示。

### 编辑页

- 字段：名称、Home、监听端口。
- 默认 Profile 的 Home 始终只读。
- 打开编辑页时按目标 ID 加载实时状态；路由运行中时 Home 与端口禁用，并提示先停止该 Profile 路由。
- 保存成功后刷新列表和目标状态，返回列表页并显示成功提示。

### 删除确认页

- 显示目标 Profile 名称和 Home。
- 文案明确说明：只删除 CC Switch 绑定和本地 token，不删除 Home、`auth.json`、`config.toml` 或会话。
- 删除成功后刷新列表；若目标为当前选中项，选择默认 Profile，默认不存在时选择第一项。

## 错误处理

- 前端基础校验在调用 API 前完成。
- Home 不存在、Home 重复、端口被其他 Profile 使用、端口已被系统监听、运行中修改受限字段等错误由后端返回，并在当前表单中显示。
- mutation 失败不得关闭 Dialog、切换视图、清空输入或改变当前选中 Profile。
- 成功操作使用简短 toast；错误以表单内反馈为主，不重复显示两份相同错误。

## 测试要求

### 前端

- 点击新建后同一 Dialog 显示表单，且不调用 `window.prompt`。
- 创建/编辑字段校验、可选端口转换、提交中状态和错误保留。
- 默认 Profile 使用 `codex-default` 正确禁用删除和 Home 编辑。
- 运行中 Profile 仅名称可编辑。
- 删除确认文案、取消和确认行为。
- Dialog 关闭重开后恢复列表页。
- mutation 正确隔离、刷新和移除 Profile 查询缓存。
- 创建后选择新 Profile，删除当前 Profile 后选择默认或第一剩余项。

### Rust

- 自动端口分配保持现有行为。
- 手动空闲端口成功创建。
- 数据库重复端口、系统已监听端口和零端口均被拒绝。
- 创建失败不残留 Profile 或空路由。
- 原子编辑任一校验失败时不修改任何字段。
- 运行中只允许改名。
- 默认 Profile 可改名和停用状态下改端口，但不可改 Home。

## 验收标准

- macOS `pnpm run dev:dump` 中点击“新建 Profile”必定在当前 Dialog 显示表单。
- 创建使用自动端口和手动端口两条路径均可成功。
- 创建、编辑和删除全程不出现第二个 Dialog 或原生 prompt。
- Profile 管理链路中不存在 `window.prompt`。
- 默认 Profile 不可删除或重绑 Home，但可改名和停用状态下改端口。
- `CC_SWITCH_DUMP_BODY=1` 的真实请求日志证明不同 Profile 的请求正文仍按监听端口和 Profile 隔离，没有跨 Home 串流。
- 所有目标测试、类型检查、格式检查、Rust check 通过。
