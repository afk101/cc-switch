# 08 — 覆盖 inline token 并保护关闭态同供应商模型

Status: resolved

**构建内容：** 修复收口复审剩余的两个行为缺口与三项规范问题：inline-table provider token 必须纳入不可逆扫描；关闭态再次选择同一供应商不得覆盖用户 model；本次新增代码不得产生 Clippy 硬错误或魔法版本值。

**受阻于：** 07 — 覆盖全部 token 路径并统一版本门禁。

## 覆盖范围

- `REQ-02`、`REQ-12`、`REQ-14`
- inline table TOML、关闭态显式同供应商、Rust standards

## Acceptance Criteria

- [x] `[model_providers]` 标准 table 与 inline table 中全部 provider 的 listener token 都不可逆处理，普通用户 token 保留。
- [x] 顶层活动 token 与 inline 非活动 provider 同时残留 listener token 的持久化 backup 解码后无法还原 token；无法无损表达时 fail closed。
- [x] 路由关闭且当前 provider 未变化时，显式再次选择同一供应商保留用户 `model` 与非路由字段；切换到不同 provider 仍允许应用目标 model。
- [x] 修复本 diff 新增的 `filter_map_bool_then`、`question_mark` Clippy 错误，并复用 backup 最小版本常量。
- [x] Codex Profile、数据库、前端、格式、Clippy 归因与全量相关回归通过。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`，两个行为均先建立公开 seam RED。
- 不通过把 inline table 强制整体删除来规避 token；保留用户 TOML 结构与普通字段。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- Home 路由字段读写与 listener token 扫描统一使用 `toml_edit::TableLike`，标准 table 和 inline table 均逐字段处理；不会删除整个 provider 表，普通 token、inline 结构和普通字段保持不变。
- inline 活动路径可用私有 token 引用无损恢复时，持久化正文删除全部精确匹配 listener token 的路径；只有 inline 非活动路径持有该 token 时保守拒绝建立备份。
- 关闭态重复选择当前供应商只更新 route 与故障转移关系，不投影供应商配置，因此保留用户模型和全部 Home 字段；选择不同供应商继续使用既有直连投影并应用目标模型。
- TDD RED 分别复现 inline listener token 可从 backup 正文还原，以及 disabled/current=A 显式 switch A 覆盖用户模型；最小实现后两者转 GREEN。
- 验证：Home 34/34、RouteManager 91/91、Codex config 79/79、Rust lib 2474 passed/2 ignored、前端 Vitest 93 files/637 tests、TypeScript typecheck、renderer production build、Rust 格式与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中本 diff 的 `filter_map_bool_then` 与 `question_mark` 已清零；仅剩未修改的 migration 1 项、body_dump 6 项、forwarder 1 项，共 8 项既有 lint。
