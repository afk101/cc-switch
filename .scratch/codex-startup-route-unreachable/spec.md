# Codex Profile 启动路由所有权收敛

## 问题陈述

Codex Profile 路由处于开启状态时，Codex Desktop、Codex CLI 或用户会正常修改 `model`、reasoning、plugins、MCP 等 `config.toml` 字段。当前启动恢复使用整份文件指纹判断 Home 是否仍由路由持有，导致任何无关字段变化都被误判为外部冲突；恢复流程随后跳过本地监听器启动，形成“界面显示开启、配置仍指向本地端口、端口却无人监听”的不可用状态。与此同时，若用户确实修改了 `model_provider` 或连接字段，启动恢复也不能擅自覆盖用户的新连接选择。

## 目标

- 启动恢复只依据实际决定 Profile 本地路由的字段判断所有权。
- 正常的模型、Desktop、插件和 MCP 变化不阻止监听器启动，也不被启动恢复覆盖。
- 用户主动改变连接路径时，CC Switch 保留用户配置并把路由状态静默收敛为关闭。
- 用户显式重新启用或切换供应商时，以操作当刻最新 Home 建立新的恢复基线。
- 路由状态、Home 实际连接路径和 runtime 状态在每次启动及生命周期操作完成后保持一致。

## 非目标

- 本次不增加 `config.toml` 后台文件监听或固定频率轮询。
- 本次不改变全局 Claude/Gemini proxy takeover 行为。
- 本次不改变 Codex Profile 私有身份状态、`auth.json` 或会话目录。
- 本次不提供 token 手动轮换产品入口。
- 本次不在 UI 展示或持久保留“外部接管”提示文案。

## 解决方案

CC Switch 将路由所有权从“整文件相等”改为“有效连接路径相等”。`model_provider` 选择当前生效的 provider；其有效 `base_url`、`wire_api` 和 listener token 共同证明请求仍经过当前 Profile。本地路由仍被持有时，启动只收敛必要路由字段并启动监听器，保留其他全部配置。当前生效路径被用户改变、配置缺失或配置结构不可可靠解析时，CC Switch 不写 Home、不启动监听器，将路由静默转为关闭并记录明确的外部接管持久化状态。

显式启用或切换供应商属于用户授权的新接管。操作以当刻最新 Home 创建版本化 backup 和字段级 ownership proof，替换旧基线；以后关闭只恢复最近一次接管前的严格路由字段，并保留关闭瞬间的所有非路由字段。

## 用户故事

1. 作为 Codex 用户，我想在路由开启期间自由切换模型，从而重启 CC Switch 后仍使用我选择的模型并能正常连接。
2. 作为 Codex Desktop 用户，我想让 Desktop、plugins 和 MCP 配置被保留，从而路由生命周期不会破坏客户端功能。
3. 作为手动维护配置的用户，我想在主动改变 provider 或连接地址后保留我的配置，从而 CC Switch 不会在重启时抢回路由。
4. 作为 CC Switch 用户，我想让路由开关反映实际状态，从而不会看到开启但端口未监听的假状态。
5. 作为 CC Switch 用户，我想在重新开启路由时以当前配置为恢复基线，从而以后关闭能回到我最新编辑过的连接配置。
6. 作为共享供应商用户，我想只有显式供应商切换才改变模型，从而普通启动和恢复不会重新应用供应商默认模型。
7. 作为升级用户，我想旧版 backup 和旧 `PROXY_MANAGED` 路由安全迁移，从而升级不会泄漏凭证或错误覆盖外部 token。
8. 作为排障人员，我想在自动收敛和失败时获得脱敏日志，从而能定位阶段而不会泄漏配置或 token。

## 可观察 Requirements

- `REQ-01`：启动恢复不得因 `model`、reasoning、Desktop、plugins、MCP、注释或其他非路由字段变化而失败、覆盖这些字段或跳过监听器。
- `REQ-02`：未发生用户显式供应商切换时，启动、恢复、启用同一供应商、健康检查和自动关闭均不得修改 `model`。
- `REQ-03`：路由所有权必须依据当前生效的 `model_provider` 以及对应有效路径上的 `base_url`、`wire_api` 和 listener token 判断，不能依据非活动 provider 的残留字段。
- `REQ-04`：当前生效连接仍属于该 Profile 时，启动后必须启动且仅启动一个对应监听器，并清除旧的启动恢复错误。
- `REQ-05`：`model_provider` 或任一严格路由字段被外部修改时，启动必须保持 Home 原样、不启动监听器，并将路由静默收敛为关闭和外部接管状态。
- `REQ-06`：配置文件缺失、非 UTF-8、TOML 损坏或结构无法可靠定位有效路由时，必须保持文件状态原样并静默收敛为关闭和外部接管状态。
- `REQ-07`：临时文件 I/O 错误不得被认定为外部接管；本次不得启动监听器，但必须保留 enabled、backup 和 ownership 状态以供后续重试。
- `REQ-08`：外部接管状态必须保留 Profile 主供应商引用和故障转移引用、清除旧 Home backup，并使后续关闭态派生对账跳过该 Home。
- `REQ-09`：外部接管自动关闭不得在 UI 保留提示或持久告警；必须写入不含 token、配置正文或敏感路径的脱敏应用日志。
- `REQ-10`：用户显式启用或切换供应商时，必须以操作当刻的 Home 建立新 backup 和 ownership proof，替换任何更早的基线，并完成用户请求的操作。
- `REQ-11`：用户显式关闭时，必须恢复最近一次明确接管前的严格路由字段，同时保留关闭瞬间最新的全部非路由字段；若 Home 已被外部接管，则不得恢复旧字段，只清理旧路由状态。
- `REQ-12`：只有用户在 CC Switch 中显式切换到另一个供应商时，才允许应用该供应商配置的模型。
- `REQ-13`：整文件指纹只能用于单次读取到原子写入之间的并发 CAS，不得再作为持久路由所有权语义。
- `REQ-14`：新版 backup 必须包含版本化字段级 ownership proof；listener token 只能以域分离的高熵摘要持久化，不得保存明文。
- `REQ-15`：新版 proof 必须支持当前 token、可证明的旧 CC Switch token 和严格匹配的 `PROXY_MANAGED` 一次性迁移，并拒绝无法证明的外部 token。
- `REQ-16`：启动所有权检查必须发生在任何可能修改 enabled Profile Home 的派生状态投影之前。
- `REQ-17`：任何流程都不得修改 `auth.json`、其他 Profile Home 或 Profile 私有身份状态。
- `REQ-18`：数据库自动关闭保存失败时，Home 必须保持原样、runtime 不得启动，route 必须保留原 enabled 状态并允许下次重试。

## Scenarios

- `SCN-01` — Given 已启用 Profile 的 `model` 被正常修改，When 应用启动，Then 模型保持不变且 listener 成功启动一次。
- `SCN-02` — Given 已启用 Profile 新增 Desktop、plugins、MCP 和未知非路由字段，When 应用启动，Then 所有字段保持且 listener 成功启动。
- `SCN-03` — Given 非活动 provider 表被修改但当前有效路径仍属于 Profile，When 应用启动，Then 不误判外部接管。
- `SCN-04` — Given `model_provider` 改为另一 provider，When 应用启动，Then Home 字节保持、listener 不启动、route 静默关闭并进入外部接管状态。
- `SCN-05` — Given 当前有效 `base_url`、`wire_api` 或 token 任一被修改，When 应用启动，Then 执行与 SCN-04 相同的保护性收敛。
- `SCN-06` — Given `config.toml` 不存在，When 应用启动，Then 不创建文件并静默关闭路由。
- `SCN-07` — Given配置非 UTF-8、TOML 损坏或活动 provider 结构异常，When 应用启动，Then 不改文件并静默关闭路由。
- `SCN-08` — Given读取配置发生权限或磁盘 I/O 错误，When 应用启动，Then route 仍 enabled、backup 保留、listener 不启动且写脱敏日志。
- `SCN-09` — Given 自动关闭的 DB save 失败，When 启动恢复结束，Then Home 未变、listener 未启动、route 仍 enabled并可重试。
- `SCN-10` — Given 已进入外部接管状态，When 下一次启动执行派生对账，Then Home 不被共享供应商配置覆盖。
- `SCN-11` — Given 外部接管后的当前 Home，When 用户显式重新启用，Then 当前 Home 成为新基线、路由启用且 listener 启动。
- `SCN-12` — Given 路由开启期间用户改变连接配置，When 用户显式切换供应商，Then 当前 Home 成为新基线且切换继续完成。
- `SCN-13` — Given 通过 SCN-11 或 SCN-12 建立新基线，When 用户关闭路由，Then 严格字段恢复为该基线且最新非路由字段保留。
- `SCN-14` — Given Home 已外部接管，When 用户显式关闭，Then 不恢复陈旧 backup，只清理并保持 Home。
- `SCN-15` — Given token secret 丢失但新版 proof 能证明 Home 中旧 token，When 应用启动，Then 只升级 token、保留非路由字段并启动 listener。
- `SCN-16` — Given旧版 backup、token 与当前 secret 不同且整文件旧证明因非路由变化失效，When 应用启动，Then 保守按外部接管静默关闭。
- `SCN-17` — Given严格匹配当前端口、responses 和 `PROXY_MANAGED` 的旧迁移路由，When 应用启动，Then 一次性升级为随机 token和新版 proof并启动 listener。
- `SCN-18` — Given外部 token 与 CC Switch token 具有相同字符串形状，When 应用启动，Then 不按格式猜测所有权并静默关闭。
- `SCN-19` — Given Home 在 ownership read 后被另一进程修改，When CC Switch 准备写入，Then CAS 拒绝覆盖最新外部写入。
- `SCN-20` — Given用户没有切换供应商，When发生启动、恢复或重新启用同一供应商，Then `model` 保持；Given用户显式切换供应商，Then允许应用新供应商模型。
- `SCN-21` — Given Profile A 发生任一恢复或外部接管场景，When流程完成，Then Profile B Home、runtime、token和私有身份状态不变。

## 关键决策

- 整文件 hash 不是路由所有权；`model` 的正常变化是本次 bug 的直接触发条件。
- `model_provider` 改变请求实际路径，因此属于外部接管，而不是普通模型变化。
- 自动流程保护用户配置；显式启用和显式供应商切换是建立新接管基线的授权。
- 外部接管静默关闭，不提供 UI 提示；日志承担排障作用。
- 不增加 watcher 或轮询，检测发生在启动及路由生命周期操作前，操作后验证本次结果。
- 临时 I/O 故障不是用户意图，不能销毁有效 backup 或 ownership 状态。
- 旧 token 所有权无法证明时 fail closed，不能用 token 长度或格式猜测。

## 实施决策

- 将关闭路径已有的字段级 projection、有效路径读取、严格字段比较与合并能力抽成启动和生命周期操作共用的单一职责接口。
- backup 采用可演进的版本 envelope；新版 ownership proof保存公共路由目标与 token摘要，不保存 token正文。
- 增加稳定的 Home ownership 状态以区分“数据库派生直连状态”和“外部接管状态”。外部接管状态保留供应商引用，但禁止自动派生写 Home。
- 自动外部接管收敛是一次 route 原子保存，不复用会写 Home 的正常关闭流程，也不创建 disable recovery WAL。
- 显式接管的新 backup 在写 Home 前持久化并进入既有可补偿生命周期，避免崩溃后丢失唯一恢复基线。
- 对 accepted ADR `0001-codex-profile-derived-state-convergence` 增加受控例外：外部接管 Home 不再是可由共享供应商自动重建的 Profile 派生状态；用户显式接管后重新进入该 ADR 的收敛域。

## 错误行为与恢复

- 外部接管、缺失或可解析层面的损坏：不写 Home，静默关闭，清旧 backup，保存 ownership 状态并写脱敏日志。
- 临时 I/O：保留 route 与 backup，不启动，日志记录文件 I/O 类别，下次重试。
- DB save 失败：不得伪装关闭，不启动 runtime，保留原状态。
- 显式启用/切换中任何步骤失败：使用既有 recovery/compensation 恢复本次操作前 Home、runtime和route；不得退回更早的陈旧 backup。
- 并发写冲突：CAS拒绝覆盖，按操作来源进入可重试错误或外部接管重新分类。

## 兼容性

- 不要求数据库表新增 token明文；backup JSON需向后读取 v1。
- v1 backup仅在当前 token、严格 legacy triple或旧整文件 target fingerprint能证明所有权时升级；否则按外部接管关闭。
- `PROXY_MANAGED` 仅接受完整严格三字段证明，一次升级后不再长期使用。
- 原版全局 takeover 的配置缺失 fail-closed 语义保持不变。
- macOS、Windows 和 Linux 均继续使用现有原子文件操作与路径规范化能力。

## 测试决策

- 首选可观察的 route 状态、Home内容和 runtime启动行为，不断言私有 helper 实现细节。
- 最高层覆盖真实应用启动顺序：先所有权/派生状态阶段，再 enabled runtime恢复。
- 使用内存数据库、临时 Home、可注入文件操作、token store和tracking runtime factory。
- 保留并扩展关闭路径现有“保留非路由字段”“严格字段冲突”“token不泄漏”测试先例。
- 定向测试后运行 Codex Profile完整单元测试、Rust全量测试、前端类型检查和生产构建。

## Test Seams

- `CodexRouteManager` 启动恢复公共接口：覆盖健康恢复、自动外部接管、I/O错误、DB失败、Profile隔离和真实两阶段顺序。
- `CodexRouteManager` 显式启用/切换/关闭接口：覆盖最新基线、模型授权和生命周期补偿。
- `CodexHomeConfigService` 公共字段级接口：覆盖活动 provider路径、严格字段、backup版本、token摘要、legacy与CAS。
- Route DAO/序列化接口：覆盖 ownership状态持久化、迁移和API可观察状态。

## 范围之外

- 实时监听运行中 Home 变化。
- 自动弹窗、toast或持久外部接管提示。
- 全局 proxy takeover重构。
- listener token用户可见轮换功能。

## 补充说明

- 诊断现场为 2026-08-12：启动在 `stage=reconcile_home category=derived_state` 失败，手动开关后同一端口立即监听。
- 原版 CC Switch 在启动严格接管失败时清除 enabled 状态，不会在配置完全缺失时凭空恢复接管；本设计延续该 fail-closed 原则。
