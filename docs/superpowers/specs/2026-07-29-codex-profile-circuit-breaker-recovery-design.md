# Codex Profile Circuit Breaker Recovery Design

## 背景

多 `CODEX_HOME` Profile 路由为每个 Profile 创建独立 `ProviderRouter` 和供应商快照。当前 Profile 选择链路无条件按熔断状态过滤快照，即使快照只有一个主供应商，也可能在请求发送前直接返回“所有供应商已熔断”。这与旧全局路由不同：旧逻辑仅在启用故障转移时使用熔断器，普通单供应商路由始终继续尝试当前供应商。

## 目标

- 恢复旧路由的策略边界：只有存在备用供应商时才启用熔断与故障转移。
- 单供应商 Profile 即使此前连续失败，也继续把后续请求发送给该供应商。
- 多供应商 Profile 保留现有熔断、HalfOpen 探测和按优先级故障转移行为。
- 保持不同 Profile 的供应商快照、熔断状态和请求内容隔离。

## 非目标

- 不新增 Profile 级“自动故障转移”数据库字段或前端开关。
- 不修改全局 Claude、Codex、Gemini 路由的既有策略。
- 不调整失败阈值、冷却时间或 HalfOpen 探测数量。
- 不清理或迁移现有内存熔断记录。

## 设计

### Profile 策略表达

`CodexRouteProviderSnapshot` 已将主供应商与备用供应商分开保存。运行时传给 `ProviderRouter` 的有序向量保持“主供应商在首位，后续元素均为备用供应商”的不变量。因此候选数量可以直接表达策略：

- 一个候选供应商：未配置故障转移，绕过熔断器。
- 两个或更多候选供应商：已配置故障转移，使用熔断器。

无需增加第二份布尔状态，避免备用列表和开关产生不一致。

### 供应商选择

`ProviderRouter::select_codex_profile_providers` 在快照只有一个供应商时直接返回该供应商，不调用 `get_or_create_circuit_breaker` 或 `is_available`。

快照包含多个供应商时保持现有循环：按顺序查询每个供应商的熔断状态，只返回当前可用的候选；全部 Open 时返回 `AllProvidersCircuitOpen`。

空快照继续返回 `NoProvidersConfigured`。正常 `CodexRouteProviderSnapshot` 不会产生空快照，但该分支保留防御性语义。

### 请求发送与结果记录

`RequestForwarder` 已根据 `providers.len() == 1` 设置 `bypass_circuit_breaker`。单供应商 Profile 经选择后进入该分支：

- 发送前不申请熔断许可。
- 失败或成功结果不更新内存熔断器和 `provider_health`。

多供应商请求继续申请许可并记录结果，无需修改转发器。

### 热切换

Profile 从多供应商切换为单供应商时，运行时只替换新请求的供应商快照。新快照只有一个候选，后续选择立即绕过旧的 Open 状态；已开始请求继续使用其原候选列表。

内存中的旧熔断记录不主动删除。若之后重新配置多个候选，既有记录可继续参与多供应商策略；路由运行时重建时仍会自然清空。

## 错误处理

- 单供应商请求不再因本地熔断返回合成 503；上游失败按实际 HTTP 状态或代理转换错误返回。
- 多供应商全部熔断仍返回 HTTP 503 和“所有供应商已熔断，无可用渠道”。
- 未配置候选供应商仍返回“未配置供应商”。

## 测试要求

在 `src-tauri/src/codex_profile/route_runtime.rs` 的运行时测试中覆盖：

1. 单供应商 Profile 达到熔断阈值后，选择结果仍包含主供应商。
2. 多供应商 Profile 的主供应商达到熔断阈值后，选择结果只包含备用供应商。
3. 多供应商 Profile 中所有候选达到阈值后，选择直接返回 `AllProvidersCircuitOpen`。
4. 同一运行时从多供应商快照切换为单供应商快照后，即使目标供应商已有 Open 记录，新请求仍能选择该供应商。
5. 现有 Profile 隔离测试继续通过，证明一个 Profile 的熔断状态不影响另一个 Profile。

## 验收标准

- 没有备用供应商的 Profile 不会再返回本地“所有供应商已熔断”503。
- 单供应商上游恢复后，用户直接重试即可恢复，无需关闭再开启路由。
- 配置备用供应商的 Profile 仍能跳过已熔断渠道并使用备用渠道。
- 不引入数据库迁移、配置格式变化或前端行为变化。
