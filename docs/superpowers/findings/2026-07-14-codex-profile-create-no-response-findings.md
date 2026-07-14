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

## 实施后全量测试回归调查（2026-07-14）

- 实施计划 Task 5 的全量前端验证使用 `pnpm exec vitest run --reporter=dot` 稳定复现 1 个失败：`tests/integration/App.test.tsx:184` 点击 `usage` 后找不到 `usage-modal`；同次运行其余 422 个测试通过。
- 失败发生在切换到 Codex 之后的既有供应商用量弹窗链路，断言前没有异步等待；失败页面仍显示 Codex 供应商列表与 Profile 上下文栏，说明 App 没有整体崩溃。
- 最近影响 App 的提交 `25bd994d` 只将 Profile 管理回调替换为 `useCodexProfileManagement` 的 `createProfile/updateProfile/deleteProfile/loadProfileState`，没有直接修改用量弹窗组件或 `onConfigureUsage` 回调。
- 当前待验证假设：新增 Profile 状态加载改变了 Codex 页面异步渲染时序，使集成测试在 ProviderList 已更新但相关 Provider 数据尚未稳定时同步点击；也可能是 ProviderList mock 通过 `providers[currentProviderId]` 取值，而 Codex Profile 选择引入了不同的当前供应商标识。下一步将从 `onConfigureUsage` 的入参和 App 用量弹窗状态反向追踪，先确认断点再讨论修复。
- 单文件复现同样稳定失败，并额外暴露 MSW 未处理的 `POST http://tauri.local/list_codex_profiles`；测试夹具没有为 App 新增的 Profile 查询提供响应。
- App 在 Codex 页面传给 `ProviderList.currentProviderId` 的值来自 `codexProfileState?.route?.currentProviderId ?? ""`，而集成测试的 ProviderList mock 再用该值执行 `providers[currentProviderId]`。Profile 查询无响应时该标识为空，点击 `usage` 实际调用 `setUsageProvider(undefined)`，因此 `effectiveUsageProvider` 仍为空，`UsageScriptModal` 按设计不会渲染。
- 由此排除“用量弹窗状态本身失效”和“点击事件未触发”；断点位于旧集成测试夹具没有跟随 Codex Profile 上下文数据源迁移，而非本次 Profile 管理 Dialog 回调接线直接破坏用量功能。下一步检查 MSW Tauri handlers 与引入 Profile 上下文的历史提交，确认应补夹具还是产品代码存在兼容缺口。
- `tests/msw/handlers.ts` 覆盖供应商、设置、MCP 等 Tauri 命令，但没有 `list_codex_profiles` 或 `get_codex_profile_state` handler；这与复现时唯一新增的 MSW 未处理请求完全对应。
- Git 历史确认 Codex Profile 上下文和 `currentProviderId` 数据源切换来自更早的提交 `0e2fd337 feat(codex): add home profile context UI`；本次 Profile 管理修复提交 `25bd994d` 没有改动 ProviderList 的 `currentProviderId` 或用量弹窗状态链路。
- 可工作的对照模式是 MSW 为 App 启动时每个必需 Tauri 查询提供确定响应，并由 `resetProviderState()` 在每个测试前恢复状态。Codex Profile 查询是该模式中唯一缺失的新数据源。
- 历史提交 `0e2fd337` 同时引入 Profile 查询和 Codex `currentProviderId` 数据源切换，却没有修改 `tests/integration/App.test.tsx` 或 `tests/msw/handlers.ts`；提交前后的集成测试片段完全相同。
- 根本原因已确认：该历史提交造成测试夹具与产品数据模型不同步。测试仍假设 Codex 当前供应商来自全局 `get_current_provider`，实际产品已改为来自选中 Profile 的 `get_codex_profile_state.route.currentProviderId`。缺少 Profile handlers 导致 mock 把空 ID 映射成 `undefined`，用量弹窗断言因此失败。
- 影响边界：这是既有全量测试债务，不是本次 Dialog 实现的产品运行时回归；真实 Tauri 验收中 Profile 状态由后端正常返回。修复应让 App 集成夹具提供默认 Profile 及其路由状态，并验证请求使用选中 Profile 的供应商，不应在产品代码中回退到全局供应商，否则会破坏多 Home 路由隔离语义。
- 等待根因确认期间完成了不依赖修复方案的后端全量验证：`cargo test --lib` 结果为 1827 通过、0 失败、2 忽略。
- 对实施计划全部显式验收项的审计显示，Task 1–4、Task 5 的目标测试、类型/格式/编译检查、无 prompt 守卫和 10 项真实 macOS 验收均已有证据；当前剩余项是修复并重新通过前端全量集成测试、重新取得最终工作区干净证据以及完成最终提交审计。

## 集成夹具修复 Brainstorming

- 用户已确认根本原因，并授权后续方案、设计和执行均采用推荐方案，不再逐项询问。
- 方案一（推荐）：在 `tests/msw/state.ts` 建立可重置的默认 Codex Profile/路由快照，在 `tests/msw/handlers.ts` 补齐列表、状态、启用路由和切换供应商命令；App 集成测试等待 Profile 当前供应商为 `codex-1` 后再操作。优点是忠实覆盖生产数据源与路由切换；新增状态仅用于测试，边界清晰。
- 方案二：只在 `tests/integration/App.test.tsx` 使用 `server.use` 临时注册两个查询 handler。改动更少，但其他 App 集成用例仍产生未处理请求，且无法覆盖后续 `enable_codex_profile_route`，测试模型会继续不完整。
- 方案三：产品代码在 Profile 状态为空时回退 `get_current_provider`。表面可让旧测试通过，但会把全局供应商状态重新混入 Profile 作用域，违反多 `CODEX_HOME` 独立路由与请求不串流的核心约束，因此明确否决。

| 技术决策                             | 理由                                                                                                                   |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| 采用共享 MSW Profile 状态模型        | App 集成测试的共享启动夹具应覆盖所有必需 Tauri 查询，并能在每个测试前恢复确定状态                                      |
| 默认 Profile ID 使用 `codex-default` | 与 Rust 和前端常量一致，避免再次产生测试专用伪 ID                                                                      |
| Profile 路由初始供应商设为 `codex-1` | 保持既有供应商夹具语义，同时验证 App 读取 Profile 路由而非全局 current provider                                        |
| Profile 路由命令更新测试状态         | 集成测试后续点击供应商会调用 `enable_codex_profile_route` 或 `switch_codex_profile_provider`，夹具必须覆盖完整用户链路 |
| 测试先等待 `current-provider` 稳定   | Provider 列表加载与 Profile 状态加载是两个独立异步查询，断言应等待真正的交互前置条件                                   |

### 设计摘要

- 数据边界：`tests/msw/state.ts` 只负责创建、克隆、读取和更新 Codex Profile 测试状态；`handlers.ts` 只负责把 Tauri 命令映射到这些状态函数；`App.test.tsx` 只表达用户路径和可见结果。
- 错误语义：未知 Profile 或未知供应商返回 404，防止夹具把错误输入静默当成功；成功启用/切换后返回 `true` 并更新目标 Profile 的路由快照。
- 回归范围：先让现有 App 集成用例稳定复现 RED，再实现夹具；验证单文件、前端全量、类型、格式、Rust 全量，以及最终 `pnpm run dev:dump` + Computer Use 真实路径。
- 规范自审确认 `CodexHomeContextBar` 的真实可见文案正是“路由已启用/路由未启用”，因此计划中的 UI 断言可直接验证 Profile 状态刷新，不依赖测试专用标识。
- 自审发现 focused Plan 的最后提交步骤重复暂存应在 brainstorming 阶段先提交的文档，并可能要求空提交；已调整为文档先独立提交、实现提交后只检查剩余差异，存在未提交的本任务变更时才创建补充提交。
- 自审发现状态函数示例只在文字中要求 JSDoc、代码块未展示注释；已在 Plan 示例中补齐中文 JSDoc，保证零背景实施者也能遵守项目规则。

## 真实验收发现：运行中 Profile 缺少停止入口

- `pnpm run dev:dump` 与 Computer Use 验收确认，运行中的 Profile 编辑页会按设计禁用 Home 和端口并提示“请先停止该 Profile 的路由后再修改”，但原界面没有任何 Profile 级停止入口；用户只能删除后重建，提示与可执行操作不闭环。
- 后端 `disable_codex_profile_route`、幂等停止状态机和前端 `codexProfilesApi.disableRoute` 已存在，缺口仅在 Query mutation、管理 hook、Dialog action 与 App 接线。
- 推荐方案是在运行中的编辑页显示“停止路由”：成功后重新读取目标 Profile 状态并解锁字段，失败时保留编辑页和错误；缓存刷新必须携带 Profile ID，不能影响其他 Home。
- TDD RED 证据：组件找不到“停止路由”按钮，hook 没有 `stopRoute`；实现后 3 个目标文件共 25 个测试通过。
- 真实 UI 证据：两个并行运行的 Profile 分别在编辑页成功停止，均显示“Codex Profile 路由已停止”，Home/端口随即解锁；后端分别记录端口 15722、15731 开始排空和 `[SRV-002] 代理服务器已完全停止`。
- 运行时证据：停止后数据库中两个路由的 `enabled` 都为 `0`，`lsof` 确认 15722、15731 均已释放；另一个 Profile 的状态和 token 未被交叉修改。

## 停止入口实施后的全量回归调查

- 首次全量前端回归中，目标组件、hook 和 query 测试全部通过；唯一失败是既有 `tests/integration/App.test.tsx` 基础流程在固定 5000ms 总时限超时。此前同一用例通过，因此先按时序抖动调查，不直接提高超时上限。
- 失败没有断言堆栈，说明用例整体超过总时限；该用例包含多次异步供应商创建、编辑、切换和复制，而本次产品差异只在 App 顶层多取得一个 `stopRoute` 回调并传入未打开的 Profile Dialog。
- 当前单一假设：`useCodexProfileManagement` 依赖整个 TanStack mutation 对象构造 `stopRoute`，可能让回调引用在 App 重渲染时变化，但 Dialog 未打开，不足以直接解释 5 秒超时；下一步先单文件重复运行并记录实际时长，判断是稳定回归还是全量并发负载造成的既有临界超时。
- 单文件替代复现通过：基础流程实际用时 2215ms，App 文件 4 个测试共 2440ms；没有出现停止路由相关错误或未处理 Profile 请求，因此排除稳定产品回归和新增 hook 调用死锁。
- 全量失败时同一基础流程耗时 5113ms，刚好越过 Vitest 默认 5000ms；同时存在 3678ms 的 SessionManager 等重型 UI 测试并行运行。机器为 12 逻辑核，项目未检出自定义 `testTimeout/maxWorkers/fileParallelism` 设置。当前证据支持“全量并发负载放大既有临界总时限”的假设，暂不改产品代码或放宽测试时限；将用不同执行配置重新验证全量行为。
- 第一个替代全量命令 `--maxWorkers=1` 在收集前失败：Tinypool 报告 `options.minThreads and options.maxThreads must not conflict`，没有执行任何测试。该方法不再重复；原因是项目确有 `vitest.config.ts`，前一次搜索模式没有命中配置内容，需要直接读取配置后选择兼容参数。
- 直接读取 `vitest.config.ts` 确认项目没有显式 worker 或 timeout 设置；Vitest 2 CLI 同时支持 `--minWorkers`、`--maxWorkers` 和 `--no-file-parallelism`。失败来自只覆盖最大值而保留内部最小值的参数组合，下一种验证同时设置最小/最大为 1，避免冲突且不改仓库配置。
- 兼容的单 worker 全量验证通过：69 个测试文件、426 个测试全部通过；App 基础流程用时 1478ms。结合单文件默认调度的 2215ms 与首次并发全量的 5113ms，根本原因已确认是机器同时运行 dev App、Computer Use 和并行测试时的瞬时调度负载放大了既有 5 秒总时限，不是停止路由实现的逻辑回归。
- 本次不修改产品代码、测试时限或 Vitest 配置：目标功能已有专用测试和真实 UI 证据；为验证而放宽固定时限会掩盖慢测试，改变 worker 配置也超出 Profile 功能范围。保留默认单文件通过与单 worker 全量通过作为两种独立证据，最终环境清理后再执行一次默认全量作为确认。
- 停止 dev App、Vite 和 Computer Use 操作并恢复 `silentStartup: true` 后，默认并发全量重新通过：69 个测试文件、426 个测试全部成功，App 基础流程耗时 4266ms。该结果与单文件、单 worker 两种证据共同闭合了瞬时调度超时调查。
