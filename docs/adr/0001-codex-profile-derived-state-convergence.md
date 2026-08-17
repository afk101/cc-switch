---
status: accepted
---

# Codex Profile 派生状态由共享供应商统一收敛

保存共享供应商时，由 Codex Profile 路由模块统一同步所有主供应商引用与故障转移供应商引用，并把数据库中的共享供应商作为权威来源。共享供应商与 Common Config 合并后的有效模型族对全部 Managed 主供应商引用具有字段级权威性：顶层 `model` 与全部顶层 `model_reasoning_*` 声明则覆盖、缺失则删除，空字符串 `model` 按缺失处理；Profile 自有扩展不属于该权威范围。故障转移供应商只更新运行中的 runtime snapshot，不投影 Home 或模型目录；External Profile 完全跳过。

交互式共享供应商保存采用全引用成功或整体补偿；手动 Sync、数据库导入和云恢复采用逐 Profile best-effort，单个失败不回滚已成功 Profile，也不回滚已经成功的数据库导入或恢复，并以脱敏的结构化 warning 报告失败 Profile。进程崩溃后的残留不引入跨文件事务日志。普通启动仍会对账其他既有派生状态，但明确保留 Home 当前 model family；模型族残留由下一次保存、切换、手动 Sync、数据库导入或云恢复收敛。

## Considered Options

- 仅保存数据库，等待用户切换供应商后再刷新 Profile Home：会让“保存成功”与实际生效状态长期不一致。
- 由前端或命令层逐项同步：会暴露锁顺序、补偿和状态分支，形成难以复用和测试的浅接口。
- 使用跨 Profile 请求屏障和持久化事务日志：可以提供更强的瞬时一致性，但会暂停新请求并显著增加恢复协议复杂度。
- 由 Codex Profile 路由模块提供单一高层接口，并通过即时补偿和按字段授权的启动对账收敛：采用此方案。

## Consequences

- 任一受影响 Profile 在交互式保存时不可写、运行时缺失或同步失败，整次保存失败并恢复已发生的变更。
- 每个 Profile 内部原子切换运行时供应商配置，但多个 Profile 不保证在同一个瞬间切换；保存成功返回前必须全部收敛。
- 路由开启态保持 listener Base URL、wire API、凭证和生命周期，只热更新 Home 有效模型族、模型目录与 runtime snapshot；在途请求继续使用进入时快照，新请求使用新快照。
- 关闭路由只恢复严格路由字段；明确同步后的有效模型族与 Profile 自有扩展继续保留。
- 手动 Sync、数据库导入和云恢复返回成功或“完成但有 Profile 同步警告”；warning 只公开 Profile 标识信息与脱敏原因。
- Profile 私有身份状态不属于派生状态，任何同步或恢复都不得复制、替换或清理它。
- Home 被判定为外部接管后不再属于可自动重建的 Profile 派生状态；只有用户显式重新接管后才重新进入本 ADR 的收敛域。
- 启动对账隔离单个 Profile 的修复失败，继续处理其他 Profile 并记录可诊断错误；它不会覆盖或删除 Home 当前 model family。
