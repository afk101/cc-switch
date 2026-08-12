# 05 — 修正审查发现的基线泄漏与 External 覆盖

Status: resolved

**构建内容：** 修复双轴代码审查发现的两个 P1：已启用显式切换不得把当前 listener 路由字段写成接管前基线；External Home 不得被共享供应商保存投影覆盖。同时复核前端调用契约并保留省略第三参数的故障转移语义。

**受阻于：** 04 — 兼容性回归闭环。

## 覆盖范围

- `REQ-08`、`REQ-11`、`REQ-14`
- 已启用普通切换、外部接管后共享供应商保存、前端既有调用契约

## Acceptance Criteria

- [x] Managed 且已启用的显式供应商切换，用旧 backup 的接管前严格字段与当前非路由字段合成最新基线。
- [x] 新 backup 与数据库导出不包含 listener token 明文；切换后关闭恢复真实接管前严格字段，而非 localhost 死路由。
- [x] External route 在共享供应商保存时不写对应 Home，同时不破坏其他 Managed/disabled Profile 的事务语义。
- [x] Toggle 测试断言省略第三参数并保留故障转移契约，不借本任务改变调用语义。
- [x] 定向、Codex Profile、数据库、前端与格式回归通过。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`，先以审查场景建立 RED。
- listener token 只能以摘要持久化；补偿必须回到本次操作前最新有效基线。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- Managed 已启用路由的显式切换先判断当前 Home 所有权；仍受管时，以旧 backup 的接管前严格字段和当前非路由字段合成新基线，真实外部接管时仍以当前 Home 建立基线。
- 共享供应商保存跳过 `home_ownership=External` 的 Home 投影，同时继续更新其他 Managed/disabled Profile 并提交供应商事务。
- Standards finding 经领域契约复核属于过期测试断言：生产 query 省略第三参数时，后端 `None` 表示保留既有故障转移；传入 `[]` 会明确清空。Issue04 的两参数修正有效，保留该断言不属于行为扩 scope。
- TDD：两个新增 manager seam 均先 RED 后 GREEN；RouteManager 84/84、Codex Profile 150/150、数据库 24/24、TypeScript typecheck、renderer production build、Rust 格式与 diff check 通过。Rust 全量 2460 passed/2 ignored，唯一 `model_pricing` 文件时序测试在全量并发中失败，隔离重跑通过。
