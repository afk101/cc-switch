# Codex Route Manager 审查修复 Findings

## 已确认根因

- `delete_custom_profile` 在调用公开 `disable` 后才删除数据库与 token，公开方法释放 Profile 锁，导致并发 `enable` 可在两步之间启动监听器并接管 Home。
- 启动流程创建 `AppState` 后没有调用 `restore_enabled_profiles`，因此已启用 Profile 不会恢复。
- `RouteRuntime::begin_draining` 只设置拒绝标记，`ProxyServer` 没有 Profile 专属 in-flight 计数及等待机制。
- 生命周期回滚使用 `let _ =` 丢弃恢复失败；`disable` 在 Home 已恢复后失败重试会因备份指纹冲突失败；删除先删数据库会令 token 删除失败无法恢复。

## 修复方向

在同一 Profile 锁内使用私有 disable 实现；让启动流程在 AppState 构造后异步恢复；为 ProxyState 增加 Profile 请求守卫、排空等待与超时；用可重试的路由状态和错误摘要保存回滚结果；删除时先删 token。

## 架构复审发现

- 三轮 manager 层补偿后，仍无法保证 runtime snapshot 与数据库 route/failover 收敛：真实 `ProxyServer::stop()` 在超时后已丢失 shutdown/handle，manager 无法重试；route 持久化回滚失败会留下分裂状态。
- `restore_enabled_profiles` 必须和用户 mutation 共享同一 Profile 锁，否则启动恢复可在 disable/delete 之后重新插入 runtime。
- drain 使用 `Notify` 时，先读取计数再注册 waiter 会丢失最后一个请求的通知，可能在无请求时等待完整超时。

## 用户确认的架构决策

| 决策 | 理由 |
|---|---|
| 将 `ProxyServer` 停止改为幂等、可观察状态机 | stop 超时后保留 shutdown 与 join handle，使下一次调用可继续等待并最终收敛。 |
| route manager 使用明确的持久化补偿状态 | runtime snapshot、route/failover 的目标状态和补偿状态必须可恢复，不能依赖失败后分散回写。 |
| 启动恢复取得同一 Profile 锁 | 防止恢复与 enable/disable/delete 交错。 |
| drain 改为无丢通知的条件循环 | 每次注册 waiter 后重新检查 in-flight，直到归零或超时。 |
