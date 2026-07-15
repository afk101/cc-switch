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

| 决策                                        | 理由                                                                                         |
| ------------------------------------------- | -------------------------------------------------------------------------------------------- |
| 将 `ProxyServer` 停止改为幂等、可观察状态机 | stop 超时后保留 shutdown 与 join handle，使下一次调用可继续等待并最终收敛。                  |
| route manager 使用明确的持久化补偿状态      | runtime snapshot、route/failover 的目标状态和补偿状态必须可恢复，不能依赖失败后分散回写。    |
| 启动恢复取得同一 Profile 锁                 | 防止恢复与 enable/disable/delete 交错。                                                      |
| drain 改为无丢通知的条件循环                | 每次注册 waiter 后重新检查 in-flight，直到归零或超时。                                       |
| 统一 lifecycle operation 状态机             | 每个 enable/switch/disable/delete 都以持久操作记录推进 phase，防止任一失败分支遗漏补偿记录。 |

## 第二次架构确认

- 用户确认：`recovery_json` 不再只是失败时附加的记录，而是每个生命周期操作的唯一操作记录。
- 记录包含 operation、phase、before、target 和无敏感 `last_error`；每一步不可逆状态变化前先持久化记录，成功后推进 phase。
- 恢复逻辑只根据 operation 与 phase 补偿或完成；无法收敛时保留记录并拒绝该 Profile 后续 mutation。

## 2026-07-15 `.codex-api` 无法关闭路由调查

- Profile `codex-api` 当前数据库状态仍为 `enabled = true`，并保留一条目标为禁用的 `recovery_json`，`last_error` 为 `CODEX_PROFILE_OPERATION_FAILURE`。
- 失败提示中的预期指纹与实际指纹不同，说明关闭流程拒绝覆盖它认为已被外部修改的 `config.toml`。
- 初次只检查顶层字段时未发现 token；继续检查活动 provider 后确认 `[model_providers.custom]` 内仍有 `experimental_bearer_token`。
- `live_backup_json` 保存了接管前配置，可用于精确恢复；其接管前内容本身已经指向旧默认 Home 的 `127.0.0.1:15721` 和 `PROXY_MANAGED`，不是 `.codex-api` 订阅原始配置。
- 当前配置仍指向 `127.0.0.1:15722/v1`，并在活动 provider 内含 `experimental_bearer_token`；15722 没有监听进程，因此是不可用的残留接管配置。
- 当前指纹既不等于备份目标指纹，也不等于备份原始指纹；失败阶段为 `disable_home_restore_failed`。
- 不能直接恢复 `live_backup_json`，因为备份原配置错误指向默认 Home 的 15721；安全恢复方案是保留 `.codex-api/auth.json` 的 ChatGPT OAuth，移除活动 provider 的本地 `base_url` 与路由 token，并清理失败路由记录。

## 现场恢复结果

- 已分别备份 `~/.codex-api/config.toml` 和 `~/.cc-switch/cc-switch.db`。
- 活动 provider 已恢复为 `name = "OpenAI"`、`requires_openai_auth = true`，不再包含 `base_url` 或 `experimental_bearer_token`；`auth.json` 仍为 `chatgpt` 模式。
- `codex-api` 路由记录已设为禁用，并清除失效的 live backup、recovery 和错误摘要。
- TOML 解析成功，数据库复查结果为 `enabled = false` 且无补偿记录，15722 无监听进程。

## 2026-07-15 第二次指纹冲突复现与恢复

- 旧版开发进程中点击供应商卡片时，`App.handleSwitchProvider` 在路由关闭态调用了 `enable_codex_profile_route`，因此 `.codex-api` 被再次写成 `127.0.0.1:15722/v1` 并注入 provider 级 `experimental_bearer_token`。
- 随后关闭路由时，当前 Home 指纹 `ca7bdff7...` 同时不匹配备份的 previous/target 指纹，补偿停在 `disable_home_restore_failed`；这不是随机指纹变化，而是“普通切换隐式启路由”与外部配置变化叠加后的安全拒绝。
- 已在再次恢复前备份配置和数据库：`~/.codex-api/config.toml.cc-switch-recovery-20260715-103029.bak`、`~/.cc-switch/backups/manual-before-codex-api-route-recovery-20260715-103029.db`。
- 第二次恢复只移除活动 provider 的 `base_url` 与 `experimental_bearer_token`，保留 `requires_openai_auth = true` 和原 `auth.json`；数据库恢复为 `enabled = false` 且 backup/recovery/error 均为空。
- 恢复后验证：`auth_mode = chatgpt`、15722 无监听、旧版 Tauri 开发进程未运行。
