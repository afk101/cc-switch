# Codex 多 Home 模型目录同步设计

## 背景

CC Switch 已支持多个 Codex Profile。供应商定义全局共享，每个 Profile 只保存当前供应商引用、故障转移策略、独立 `CODEX_HOME` 和监听端口。

Codex 的自定义模型列表由 `config.toml` 顶层字段 `model_catalog_json` 指向的 JSON 文件提供。当前实现把 `cc-switch-model-catalog.json` 当作供应商直连切换时附带写入当前 Home 的辅助文件：路由开启、路由开启态切换供应商、共享供应商编辑和启动恢复都不会完整投影该文件。因此新的 Home 可能缺少目录，同一共享供应商的多个引用 Home 也可能保留不同版本。

## 目标

- `Provider.settings_config.modelCatalog` 是模型目录唯一真相源。
- 每个 Profile Home 保存独立的 `cc-switch-model-catalog.json` 派生副本。
- 同一供应商的模型映射修改同步到所有以该供应商为主供应商的 Profiles。
- Profile 切换供应商只改变该 Profile 的目录投影。
- 默认 `~/.codex` 与其他 Home 使用完全相同的规则。
- 首次开启路由、路由开启态切换和启动恢复都能生成或修复目录。
- 多 Home 更新采用全有或全无的补偿语义。
- Codex 无需热重载；更新在各实例下次启动时生效。

## 非目标

- 不使用软链接。
- 不把所有 Home 无条件指向同一个全局目录。
- 不改模型目录 JSON schema、模型映射表单或现有国际化提示。
- 不让 Profile `/models` 读取默认 Home；该端点继续返回空模型列表，避免跨 Home 泄漏。
- 不删除未被引用的旧 `cc-switch-model-catalog.json`。
- 不改变仅作为 failover 的供应商对应的当前 Home 模型目录。
- 本设计只协调模型目录投影；供应商其他字段继续遵循现有 Provider 与 Profile 生命周期。

## 权威状态与派生状态

```text
共享 Provider.settings_config.modelCatalog（唯一真相源）
        │
        ├── Profile A.current_provider_id == Provider.id
        │     └── <home-a>/cc-switch-model-catalog.json
        │
        └── Profile B.current_provider_id == Provider.id
              └── <home-b>/cc-switch-model-catalog.json
```

每个 Home 的 `config.toml` 使用相对指针：

```toml
model_catalog_json = "cc-switch-model-catalog.json"
```

相对路径保留现有 WSL、符号链接和 Home 可迁移性。JSON 副本是可丢弃、可重建的缓存，不参与供应商配置共享。

## 组件边界

### Codex 配置投影核心

`src-tauri/src/codex_config.rs` 提供无副作用的目录生成与指针投影能力：

- 从供应商 settings 和供应商原始 config 生成模型目录 JSON；
- 对目标 Home 当前 config 只设置或移除 `model_catalog_json`；
- 目录存在时写入 CC Switch 相对文件名；
- 目录不存在时只移除文件名为 `cc-switch-model-catalog.json` 的 CC Switch 指针；
- 不在该窄函数中修改 `web_search`、路由字段或凭证。

### 单 Home 计划

`CodexHomeConfigService` 负责单个 Home：

- 读取 config 和目录快照；
- 构造 `CodexModelCatalogProjectionPlan`；
- config 目标内容与当前内容相同时不创建 config 写入动作；
- 目录存在时创建 JSON 原子写计划；
- 目录不存在时保留磁盘 JSON，仅生成可选的指针移除计划；
- 应用前校验原指纹，补偿前校验目标指纹。

### 批量投影执行器

新增聚焦的批量执行单元，负责：

- 接收已按 Profile ID 排序的单 Home 计划；
- 预检全部计划后按顺序应用；
- 失败时按反向顺序恢复已应用计划；
- 成功时返回持有全部已应用计划的 receipt，供后续供应商提交失败时反向恢复；
- 返回失败 Profile/Home 和补偿错误，不包含配置正文或 token。

批量同步与启动自愈的稳定错误前缀定义在 `src-tauri/src/codex_profile/constants.rs`，不在执行器或 manager 中散落字符串常量。

它不查询数据库、不选择供应商，也不管理 runtime。

### Route Manager 协调

`CodexRouteManager` 负责：

- 从 `list_profiles()` 与 `get_route()` 筛选 `current_provider_id` 等于目标供应商的 Profiles；
- 只选择主供应商引用，不复用包含 failover 的删除保护查询；
- 使用一个目录同步锁串行化供应商编辑、路由启停和 Profile 供应商切换；
- 按 Profile ID 获取 Profile 锁；
- 调用单 Home 计划和批量执行器；
- 把目录补偿纳入现有路由/runtime/DB 补偿链。

### Provider 更新入口

通用 `update_provider` Tauri 命令改为 async。非 Codex 应用保持原调用。Codex 更新通过 Route Manager 的目录投影事务执行既有 `ProviderService::update()` 提交动作：

1. 保存原供应商快照；
2. 校验并规范化新供应商设置；
3. 锁定所有主引用 Profiles；
4. 准备并应用全部 Home 投影；
5. 提交共享供应商更新；
6. 提交失败时恢复 Home，并确保共享供应商仍为旧值；
7. 任一补偿失败时返回组合错误。

`ProviderService` 继续拥有供应商校验、规范化、共享 DB 和既有默认 live 语义；目录协调器不复制这些规则。

## 行为矩阵

| 场景 | config 指针 | JSON 文件 | 影响范围 |
|------|-------------|-----------|----------|
| 新 Home 选择带目录供应商 | 设置 CC Switch 相对指针 | 生成 | 当前 Profile |
| 新 Home 首次开启路由 | 设置 CC Switch 相对指针 | 生成 | 当前 Profile |
| 已有正确指针，编辑模型映射 | 不改 config | 重写 | 所有主引用 Profiles |
| 路由开启态切换到另一带目录供应商 | 保持/设置指针 | 重写为目标供应商 | 当前 Profile |
| 切换到无目录供应商 | 仅移除 CC Switch 指针 | 保留 | 当前 Profile |
| 编辑无任何主引用的供应商 | 不改 | 不改 | 无 Home |
| 供应商仅作为 failover | 不改 | 不改 | 无 Home |
| config 指向用户自定义目录且目标无目录 | 保留自定义指针 | 不改 | 当前 Profile |
| config 指向用户自定义目录且明确选择带目录供应商 | 切换为 CC Switch 相对指针 | 生成 | 当前 Profile |

## 数据流

### 路由开启

```text
获取目录同步锁
  → 获取 Profile 锁
  → 读取 Home 快照
  → 准备目标供应商目录计划
  → 准备路由 config 计划
  → 应用目录与 config
  → 启动并健康检查监听器
  → 保存 route/failover
  → 失败时按现有顺序补偿 runtime、route、config、catalog
```

### Profile 供应商切换

路由关闭时继续使用直连计划，但目录生成复用新的窄投影核心。路由开启时先应用当前 Home 的目标目录计划，再替换 runtime snapshot 和 DB 引用。后续任一步失败时恢复目录计划以及现有 runtime/route 快照。

### 编辑共享供应商

```text
无副作用校验/规范化
  → 获取目录同步锁
  → 查询并排序主引用 Profiles
  → 获取全部 Profile 锁
  → 准备全部 Home 计划
  → 预检全部指纹与序列化结果
  → 逐个原子应用
  → 提交共享供应商
  → 成功后释放锁
```

若写入或提交失败，按反向顺序恢复已应用 Home。错误包含失败 Profile 名称、Home 路径和补偿结论。

### 启动自愈

CC Switch 启动时遍历所有存在 `current_provider_id` 的 Profiles，按共享 DB 当前供应商重新生成目录：

- 缺失文件会补建；
- 过期文件会重写；
- 缺失或错误的 CC Switch 指针会修复；
- 无目录供应商会移除 CC Switch 指针并保留 JSON；
- 单 Profile 失败以专用模型目录错误前缀写入其 route `last_error`，不阻塞其他 Profiles 和应用启动；成功时只清理相同前缀的旧错误，不覆盖其他路由生命周期错误。

启动自愈完成后，再按现有流程恢复已启用监听器。

## 并发与锁顺序

所有可能改变 Profile 供应商或目录的操作采用统一顺序：

1. 目录同步锁；
2. 按 Profile ID 升序排列的 Profile 锁；
3. 文件计划应用；
4. runtime 与持久化变更。

单 Profile 操作也遵循相同外层目录同步锁。禁止先持有 Profile 锁再等待目录同步锁，避免与批量供应商更新形成反向锁序。

## 文件所有权与外部修改

- `cc-switch-model-catalog.json` 是 CC Switch 管理文件，历史手工内容可以被 DB 投影覆盖。
- 指向其他文件名的目录指针属于用户，移除目录时不改动。
- 所有 config 目标内容从当次读取内容做字段级变换，保留无关 TOML 字段与注释。
- 应用前重新读取并校验原指纹；发现并发外部修改时拒绝覆盖。
- 补偿只在当前文件仍匹配本次目标指纹时执行；否则返回补偿未收敛错误。

## 错误处理

- 预检失败：不写任何 Home 或 DB。
- 第 N 个 Home 应用失败：反向恢复前 N-1 个 Home。
- 供应商提交失败：恢复全部 Home，并保留旧供应商记录。
- runtime/route 持久化失败：沿用现有补偿状态机，并追加目录恢复结果。
- 启动自愈失败：记录单 Profile 错误，继续其他 Profiles。
- 错误和日志不输出 token、auth、config 正文或目录正文；只允许 Profile ID/名称、Home 路径、阶段和指纹。

## 测试要求

### 配置与单 Home 单元测试

- 从供应商 config 生成目录，但只变换目标 Home 的目录指针。
- 已有正确指针时 config 不产生写计划。
- 无目录时移除 CC Switch 指针并保留磁盘 JSON。
- 无目录时保留用户自定义指针。
- 带目录供应商明确接管用户自定义指针。
- 应用和恢复分别拒绝不匹配的原/目标指纹。

### 批量同步测试

- 同一供应商的多个主引用 Home 全部获得相同目录内容。
- 不同供应商和仅 failover 引用不更新。
- 默认 Home 与自定义 Home 走相同分支。
- 第二个 Home 失败会恢复第一个 Home。
- DB 提交失败会恢复全部 Home 并保留旧供应商。
- 补偿期间发生外部修改会返回组合错误而不覆盖。

### Route Manager 测试

- 空 Home 首次开启路由生成目录与指针。
- 路由开启态切换供应商更新目录。
- 路由/运行时/持久化失败恢复目录。
- 并发供应商编辑与 Profile 切换最终与 DB 引用一致且不死锁。
- 启动自愈补建缺失文件，单 Profile 失败不阻塞其他 Profile。

### 回归测试

- 现有路由启停、配置指纹、字段级恢复和补偿测试继续通过。
- 保留旧的“单 Home 直连计划只写目标 Home”测试，新增批量协调测试覆盖“只更新主引用该供应商的 Homes”。
- Profile `/models` 继续返回空列表。
- 前端无需新增交互；现有供应商保存与 Profile 刷新流程继续工作。

## 验收标准

1. 在 `~/.codex-api` 首次开启带自定义模型供应商的路由，无需手工复制或软链接即可生成目录和指针。
2. 两个 Profiles 引用同一供应商时，编辑一次模型映射会更新两个 Home 的目录；重启各自 Codex 后模型列表一致。
3. 两个 Profiles 引用不同供应商时，编辑其中一个供应商不会改变另一个 Home。
4. 切换到无目录供应商后 TOML 不再引用 CC Switch 目录，但旧 JSON 仍保留。
5. 任一引用 Home 不可写时，供应商保存失败，其他 Home 和共享供应商保持旧状态。
6. 删除目录文件后重启 CC Switch，会从 DB 自动补建；单个损坏 Home 不影响其他 Profile 恢复。
7. 不存在跨 Home 请求状态、模型目录或默认 Home 配置泄漏。
