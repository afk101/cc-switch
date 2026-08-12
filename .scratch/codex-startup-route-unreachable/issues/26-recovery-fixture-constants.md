# 26 — 恢复测试夹具统一 operation 常量

Status: resolved

**构建内容：** 修复 Gold Standards 剩余 5 处恢复记录测试夹具裸字符串，并建立覆盖全部新增 recovery operation 夹具的扫描门禁。

**受阻于：** 25 — 启动恢复保留用户 MCP。

## Acceptance Criteria

- [x] 所有新增/改写的 `RouteRecoveryRecord.operation` 测试夹具使用既有 enable/disable/switch/delete 常量。
- [x] 全文件扫描不再发现生产或新增测试中的 recovery operation 协议裸字符串；历史 JSON 兼容文本可明确排除。
- [x] 对应恢复/兼容测试、Codex Profile、fmt、diff check 通过。

## 执行约束

- 不改生产行为，不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 全文件扫描共定位并修复 15 处类型化 `RouteRecoveryRecord.operation` 夹具裸字符串，统一使用既有 enable/disable/switch 常量；当前没有需要修改的 delete 夹具。
- `switching_with_invalid_recovery_keeps_json_and_has_no_side_effects` 中两条原始 JSON 保留：它们专门构造历史线格式、未知字段和 operation/phase 错配；改成类型化序列化会提前消除待测非法输入，代码旁已记录中文排除理由。
- 扫描门禁覆盖 `operation: "...".to_string()`、与裸字符串比较及 `String::from("...")` 三类形式，类型化夹具结果为零；仅上述两条有据可查的 raw JSON 兼容夹具命中。
- Route Manager 135/135、Codex Profile 212/212 通过；`cargo fmt --all -- --check`、`git diff --check` 通过。
