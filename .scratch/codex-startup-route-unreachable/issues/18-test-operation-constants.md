# 18 — 测试故障注入统一 recovery operation 常量

Status: resolved

**构建内容：** 修复 Standards 终审剩余两处测试 helper 裸字符串，避免 operation tag 改名后故障注入失效、测试假绿。

**受阻于：** 17 — 收口阶段推进与异步尾窗。

## Acceptance Criteria

- [x] disable/enable 故障注入 helper 使用既有 recovery operation 常量，不再比较裸字符串。
- [x] 对应原子性定向测试仍能真实触发故障并通过。
- [x] fmt、diff check、相关 Codex Profile 回归通过。

## 执行约束

- 不改生产行为，不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- `InterfereAfterDisablePreparedPersistence` 与 `AtomicEnableSavePersistence` 已分别改用 `CODEX_ROUTE_RECOVERY_OPERATION_DISABLE`、`CODEX_ROUTE_RECOVERY_OPERATION_ENABLE`，未修改生产行为。
- 两条原子性定向测试各真实执行 1 条并通过；Route Manager 119/119、Codex Profile 195/195 通过。
- `cargo fmt --all -- --check`、`git diff --check` 及 recovery operation 裸字符串扫描通过。
