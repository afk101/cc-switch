# Codex Profile 新建无响应调查

## 用户提供的第一手信息

- 调查日期：2026-07-14
- 现象：点击“新建 Profile”后没有反应。
- 精确复现路径：进入 Codex 页面，在新增的 Profile 栏点击最右侧管理按钮，打开“管理 Codex Profile”弹窗，再点击弹窗底部的“新建 Profile”。
- 截图显示：弹窗当前只列出“默认 Codex”，监听端口为 `15721`；底部“新建 Profile”按钮处于可点击的蓝色状态。
- 复现稳定性：每次点击均必现，没有偶尔成功的情况。
- 预期行为：点击后应在当前“管理 Codex Profile”弹窗内部显示新建 Profile 表单，不应再打开一个独立弹窗。
- 当前目标：确认无响应发生在哪一层及其根本原因；在根因确认前不修改实现代码。

## 待确认信息

- 已完成：用户确认每次必现；代码与上游资料确认该行为不依赖特定 Profile，而是 macOS Tauri/WKWebView 的固定运行时行为。

## 调查记录

- 用户提供启动/运行日志：
  - `Browserslist: browsers data (caniuse-lite) is 8 months old`：前端兼容性数据库过期提示，暂未发现与点击事件直接相关。
  - `处理托盘菜单事件: show_main`：托盘显示主窗口事件正常进入后端。
  - `error messaging the mach port for IMKCFRunLoopWakeUpReliable`：macOS 输入法通信告警，暂未发现与 Profile 创建直接相关。
- 用户提供的日志中，点击“新建 Profile”后没有出现新的 Profile 命令调用或错误输出。该证据只能缩小调查范围，尚不足以确认根因。
- Git 工作区在调查开始时除本 findings 文件外无未提交改动。
- 最近相关提交为 `0e2fd337 feat(codex): add home profile context UI`，该提交首次引入 Profile 上下文栏和管理弹窗，是本次回归的主要变更范围。
- 本地历史记忆索引未找到 cc-switch/Codex Profile 的额外调查记录，后续以当前仓库代码和运行时证据为准。
- `CodexProfileManagerDialog.tsx` 中“新建 Profile”按钮仅调用外部传入的 `onCreate`，组件内部只有端口编辑状态，没有新建表单的显示/隐藏状态，也没有新建表单 JSX。
- 初始提交中的组件实现与当前实现一致，说明该行为不是后续改坏，而是首次实现时就缺少弹窗内新建 UI。
- `CodexProfileManagerDialog.test.tsx` 只验证默认 Profile 可改端口、不可删除/重绑，没有点击“新建 Profile”后的可见行为测试，因此现有测试无法发现该问题。
- `App.tsx` 将管理弹窗的 `onCreate` 实现为连续调用 `window.prompt("Profile 名称")` 与 `window.prompt("CODEX_HOME 路径")`，没有设置任何管理弹窗内部的新建表单状态。
- 这与用户确认的产品预期（在当前管理弹窗内展示表单）直接不一致。
- 完整调用链中，“新建 Profile”按钮没有禁用条件或提前返回；点击后第一条同步语句就是 `window.prompt("Profile 名称")`。
- `window.prompt` 仅出现在这次新增的 Codex Profile 创建/重绑逻辑中；仓库其他创建与编辑交互使用 React 受控表单和项目 Dialog 组件。
- 反向调用链：用户点击按钮 → `CodexProfileManagerDialog` 调用 `onCreate` → App 同步调用 `window.prompt` → 任一结果为空时直接 `return` → `codexProfilesApi.create` 不会执行 → 后端没有 Profile 命令日志。该链路与用户提供的现象一致。
- 调查遵循 root-cause-tracing：继续向源头追踪到首次 UI 设计，而不是在后端创建命令或按钮样式处处理症状。
- 当前项目使用 Tauri `2.8.x`（Rust `tauri = 2.8.2`，前端 API `2.8.0`，CLI `2.8.1`），macOS 运行时由 Wry/WKWebView 承载。
- Tauri 官方 issue [#14051](https://github.com/tauri-apps/tauri/issues/14051) 明确记录：macOS 上 `window.prompt()` 不显示且直接返回 `null`。
- Tauri 官方 Wry issue [#584](https://github.com/tauri-apps/wry/issues/584) 说明 macOS 上 `window.prompt()` 未被注入支持。
- Tauri 官方插件 issue [#2145](https://github.com/tauri-apps/plugins-workspace/issues/2145) 记录 dialog 插件缺少文本输入 prompt；`window.prompt` 虽存在但不产生 UI。

## 模式对比

- 可工作参考：`src/components/prompts/PromptFormModal.tsx`。
- 参考实现完整使用 React 受控状态、项目 `Input`/`Button`、显式保存中状态、禁用校验和异步错误边界。
- 损坏实现没有任何 Profile 名称/Home 路径输入控件，没有“创建中”状态或前端校验反馈，仅依赖 `window.prompt` 的同步返回值。
- 共同依赖（React、项目 Dialog/Input/Button、异步 API 与 toast）在仓库中均已存在；创建 Profile 不需要引入新的 UI 库或原生能力。
- 关键差异不是后端 API 或数据库，而是 Codex Profile 首次 UI 实现绕过了项目已有受控表单模式。
- 原实施计划 `2026-07-14-codex-profile-ui.md` 的 Task 4 仅指定“默认 Profile 可修改端口但不可删除或重绑”的组件测试，没有“点击新建后出现输入表单并能提交”的测试。
- 计划虽然在后端命令层要求 `create`、`rename`，但前端 UI 步骤没有细化创建/重命名的交互和验收；这给临时使用 `window.prompt` 留下了未受测试约束的空白。
- UI 设计文档明确要求管理弹窗支持创建、重命名；新建时可填写名称、Home 和手动端口，并展示后端可定位错误。当前实现不仅运行时失效，也没有完成这部分设计契约。
- 当前唯一组件测试执行结果：`1 passed`。这证明现有自动化验证在用户路径损坏时仍会全绿。

## Brainstorming 补充发现

- 前端 `codexProfilesApi.create(name, homePath)` 与后端 `create_codex_profile(name, homePath)` 当前不接收端口；Repository 创建时通过 `allocate_port()` 自动分配。
- `rename_codex_profile` 和前端 `codexProfilesApi.rename` 已存在，但管理弹窗没有名称编辑入口。
- “重新绑定 Home”同样依赖 macOS 不支持的 `window.prompt`，因此修复创建交互时需要决定是否一并恢复该既有管理能力。
- 若要支持设计文档中的“提交前手动填写端口”，推荐扩展创建命令接收可选端口并在 Repository 内一次性验证、创建；不推荐前端先创建再调用 `updatePort`，因为第二步失败会留下与用户输入不一致的半成品 Profile。

## Brainstorming 技术决策

| 决策                             | 理由                                                                                                                                                           |
| -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 暂不直接实现                     | brainstorming 硬门槛要求设计获批后才能进入实现                                                                                                                 |
| 复用现有 findings                | 本任务属于 systematic-debugging 后的修复场景，不新建 findings                                                                                                  |
| 完整补齐 Profile 管理能力        | 用户确认本次不只修复创建按钮；同时补齐重命名、自定义 Home 重绑、创建时可选手动端口、删除确认，并移除全部 `window.prompt`                                       |
| 单 Dialog 内部切换列表页与编辑页 | 用户确认不叠加新弹窗；创建或编辑时替换当前 Dialog 内容，保存/取消后返回列表，避免列表内同时展开多个表单造成拥挤                                                |
| 新建端口为空时自动分配           | 用户确认端口输入为可选字段；填写时由后端在同一次创建操作中验证并使用，避免创建后更新端口产生半成品状态                                                         |
| 运行中仅允许改名                 | 用户确认编辑页读取目标 Profile 实时状态；路由运行时 Home 与端口禁用并提示先停止路由，名称仍可修改                                                              |
| 删除确认使用同 Dialog 确认页     | 用户确认不叠加第二个确认弹窗；确认页明确不会删除 Home、auth.json、config.toml 或会话                                                                           |
| 创建后返回列表并选中新 Profile   | 用户确认 Dialog 保持打开，刷新列表和主页面上下文，并显示简短成功提示，方便继续管理                                                                             |
| 表单错误就地展示                 | 用户确认名称/Home 去除首尾空格，手动端口限制为 1–65535 整数；后端错误保留输入并显示在当前页，提交中禁用重复操作                                                |
| 选择专用 Profile 管理工作流方案  | 用户选择方案 1；以管理 Dialog 协调四种内部视图，表单和 mutation 使用专用边界，后端扩展原子可选端口创建                                                         |
| 组件架构获批                     | Dialog 仅协调 `list/create/edit/delete-confirm`；专用表单负责字段与校验；mutation 控制层负责 API、缓存刷新和选择；Repository 负责最终校验与原子创建            |
| 创建与编辑采用原子命令           | 用户批准创建接收可选端口；新增原子 `update_codex_profile` 统一校验名称/Home/端口后一次更新，避免串联 rename/rebind/updatePort 造成部分成功；现有窄命令保留兼容 |
| 删除沿用 RouteManager 生命周期   | 用户批准删除继续委托现有 `delete_custom_profile`；删除当前项后选择默认或首个剩余 Profile，并刷新列表/状态                                                      |
| 单 Dialog 四种页面交互获批       | 列表使用统一编辑入口；创建含名称/Home/可选端口；编辑遵循默认与运行中限制；删除确认显示完整保留文件文案；关闭时丢弃输入并重置列表                               |

## Brainstorming 架构约束

- `CodexProfileRepository::allocate_port()` 已同时检查数据库已保留端口和 `127.0.0.1` 上的操作系统端口占用；手动端口应提取并复用同一端口可用性校验。
- `create_codex_profile_with_empty_route` 已提供 Profile 与空路由记录的事务写入，扩展可选端口不应破坏该原子边界。
- 当前项目 Dialog 组件允许在同一个 `DialogContent` 内替换 Header、Body、Footer；实现列表/创建/编辑/删除确认视图不需要嵌套 Dialog。
- 为遵守单一职责，管理 Dialog 应只协调视图状态；表单字段、校验与提交状态应放在专用子组件或专用 hook 中，避免继续扩大 `App.tsx` 的内联回调。
- 现有查询层普遍在 `src/lib/query/*.ts` 使用 TanStack `useMutation` 与 query invalidation；Profile mutation 控制层应扩展 `src/lib/query/codexProfiles.ts`，而不是另造不一致的请求机制。
- Rust 默认 Profile ID 常量为 `codex-default`，当前前端弹窗却硬编码比较 `"default"`，导致默认 Profile 的删除/重绑按钮保护可能失效。前端必须在 `src/config/constants.ts` 定义并统一使用 `CODEX_DEFAULT_PROFILE_ID`，并增加默认规则回归测试。
- Rust 命令注册位于 `src-tauri/src/lib.rs`，新增原子 `update_codex_profile` 需要同步注册；现有命令模块导出通过 `commands/mod.rs` 处理。
- `src/lib/query/codexProfiles.test.ts` 当前只覆盖查询键隔离；mutation 缓存刷新、创建后选择和删除后回退需要新增 hook 测试。
- DAO 的 `create_codex_profile_with_empty_route` 已用 SQLite transaction 写入 Profile 与空路由；`update_codex_profile` 已一次更新名称、Home、端口和时间戳，Repository 可在全部校验通过后复用该整体更新。
- Profile 命令由 `commands/mod.rs` 通配导出，并在 `src-tauri/src/lib.rs` 的 `generate_handler!` 明确注册；新增命令无需创建新的命令模块。

## 文档自审记录

- Spec 与 Plan 第一轮已覆盖组件、数据流、错误处理、前后端测试和真实 Tauri 验收。
- 发现前端端口常量名 `MIN_PORT/MAX_PORT` 作用域过宽，需改为 `CODEX_PROFILE_MIN_PORT/CODEX_PROFILE_MAX_PORT`。
- 发现计划测试示例存在“用注释描述断言”的不完整代码，需在提交前替换为具体可执行测试或精确逐步动作。
- 发现原子 update API 测试使用 `expect.any(Object)` 过宽，需改为精确 DTO payload 断言。
- 发现 mutation hook 计划只列无返回类型签名，需明确 `UseMutationResult` 的结果、错误和变量类型及实际 `useMutation` 结构。
- 发现无 prompt 搜索预期依赖 `rg` 状态 1，需改为显式 shell 条件并以状态 0 表示守卫通过，避免把预期无匹配记录成失败操作。
- 已完成上述修正：常量改为 `CODEX_PROFILE_MIN_PORT/MAX_PORT`，测试示例改为实际交互与断言，API 使用精确 payload，mutation hooks 展开真实结构，无 prompt 守卫成功时返回 0。
- 第二轮禁止占位符扫描无匹配；`git diff --check` 通过；工作区仅包含本次 findings、spec、plan 三份文档。
- 第一次规格覆盖脚本因查找字面量“不串”失败，揭示 Spec 未显式复述用户的跨 Home 请求内容隔离要求；调整方案为先补充隔离回归约束，再使用语义关键词复核，不重复原命令。
- 已在 Spec 增加请求正文、凭证、供应商快照和运行时状态不得跨 Profile 串流的约束，并在 Plan 增加双 Home 可识别请求的日志验收。
- 第二种语义覆盖检查通过，全部批准需求均同时出现在 Spec 与 Plan；最终 `git diff --check` 通过。
- 提交前将按 verification-before-completion 重新执行完整文档校验，并按 generate-commit 从暂存差异生成 `docs` 类型 Conventional Commit；不会把文档完成误报为业务修复完成。
- 已仅暂存本次三份文档；暂存统计为 3 files / 976 insertions。一次性完整 cached diff 输出被工具截断，改用按文件和行段读取暂存内容，避免重复相同的截断方式。
- 已逐文件完整读取暂存版 findings 与 spec，未发现新的矛盾、占位符或范围漂移；继续按行段复核 plan。
- 分段读取完整 plan 后发现部分测试示例仍引用未定义 helper/fixture（Repository、QueryClient、userEvent、Profile fixtures）；需改为自包含 setup，并为新增 TypeScript helper 添加 JSDoc 后重新暂存。
- 已补全 Repository 数据库/Home/fake 端口 setup、Tauri invoke mock、QueryClient wrapper、Profile 固定 fixture、userEvent 和 Dialog renderer；表单校验 helper 已展开实际函数体，运行中编辑测试限定单一 Profile 避免多按钮歧义。
- 提交前首次 Prettier 检查发现 findings 与 plan 需要格式化；采用工具建议的 `prettier --write` 机械修正后再检查。
- 首次综合校验脚本在 zsh 中使用变量名 `path`，意外覆盖 zsh 特殊 PATH 数组，导致后续 `rg` 找不到；改用 bash 和变量名 `doc_path`，不重复原脚本。
- 替代验证通过：Prettier 确认三份文件格式一致；bash 综合校验确认仅暂存三份目标文档、无未暂存改动、路径合法、cached diff 无空白错误、无占位符，且 Spec/Plan 同时覆盖全部批准需求。

## 假设与验证

- 初步方向：故障位于前端按钮事件处理或弹窗状态切换链路。
- 单一假设：按钮事件本身正常触发；根本原因是首次 UI 实现用 macOS Tauri/WKWebView 不支持的 `window.prompt` 代替设计要求的弹窗内受控表单，`prompt` 返回 `null` 后 App 静默提前退出。之所以未被发现，是实施计划和测试没有覆盖“点击新建 → 显示并提交表单”的行为。
- 已确认部分：App 层 `onCreate` 确实没有驱动管理弹窗内部新建表单。
- 已确认运行时行为：Tauri 2 macOS/WKWebView 的 `window.prompt` 不显示并返回 `null`；这会触发当前实现的静默提前返回。
- 最小诊断测试结果：点击“新建 Profile”后 `onCreate` 被调用一次，测试通过；已排除按钮遮挡、Button 组件失效和 Dialog 事件吞掉等方向。
- 第一次 findings 更新补丁因段落上下文过时未匹配；调整为先读取当前文件、再按实际上下文精确追加，第二种方法成功，没有重复失败操作。

## 根本原因

- 直接根因：`App.tsx` 的 Profile 创建处理器使用两个 `window.prompt` 收集名称和 Home 路径。macOS Tauri 2/WKWebView 不展示 `window.prompt`，并直接返回 `null`；随后当前代码在 `if (!name || !homePath) return` 静默退出，因此不会显示表单、不会调用后端、也不会产生错误日志。
- 设计根因：`CodexProfileManagerDialog` 没有实现设计文档要求的弹窗内受控创建表单；首次实现用浏览器同步 prompt 替代了项目既有的 React 表单模式。
- 过程根因：实施计划与组件测试只覆盖默认 Profile 的端口/删除/重绑规则，没有覆盖“点击新建 → 显示名称/Home/端口表单 → 提交创建”的完整用户行为，因此不完整实现仍能通过全部既有测试。
- 排除项：按钮点击事件正常；后端创建命令和数据库尚未被调用；Browserslist 过期提示与 macOS 输入法告警不是本问题原因。
- 清理结果：临时诊断测试已移除；当前工作区除本 findings 文件外没有新增未提交代码，`git diff --check` 通过。
