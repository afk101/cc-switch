# 07 — 覆盖全部 token 路径并统一版本门禁

Status: resolved

**构建内容：** 修复最终 Spec 复审发现的剩余边界：备份脱敏必须覆盖活动、非活动 provider 与顶层所有可持久化 token 路径；未知未来 backup version 必须在所有消费入口 fail closed。

**受阻于：** 06 — 消除备份中可逆 listener token。

## 覆盖范围

- `REQ-14`、`REQ-15`
- v3 backup 安全、未来版本降级保护

## Acceptance Criteria

- [x] 构建/重建 backup 时扫描所有潜在 `experimental_bearer_token` 路径；凡值等于本 Profile listener token，均不得以明文或可逆编码进入持久化内容。
- [x] 非活动 provider、顶层与活动 provider 同时残留 listener token 的解码级回归通过；无法安全表达的多路径状态应拒绝建立 backup，不得悄悄丢语义。
- [x] 版本门禁统一限制 v1/v2/v3；未知未来 version 在关闭、启用补偿、切换基线与启动恢复中都 fail closed，Home/route/recovery 不发生未授权副作用。
- [x] v1/v2 迁移、v3 sentinel、普通用户 token、Profile 隔离和补偿保持通过。
- [x] Codex Profile、数据库、前端、格式与全量相关回归通过。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`；每个公开 seam 先建立 RED。
- 不通过删除未知用户 token 或整个 provider 表来规避；只处理精确匹配本 Profile listener token 的路径。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- v3 备份构建与重定位会扫描顶层和全部 provider 表，仅删除精确等于当前 Profile listener token 的字段；普通用户 token 保留。活动路径能证明引用语义时，关闭从私有 token store 恢复活动字段，其他路径继续按当前 Home 非路由状态保留；只有非活动路径残留 listener token、无法无损表达时拒绝建立备份。
- 统一 decode 门禁只接受 v1/v2/v3，版本范围常量位于 `codex_profile/constants.rs`。显式关闭、启用恢复、显式切换和启动恢复均在 Home、route、runtime 或 recovery 副作用前拒绝 v4；启动仅写脱敏应用日志并阻断本次 runtime，不持久化 route 错误。
- TDD RED 分别复现“解码 previous_content 可还原非活动/顶层 listener token”和“v4 可 serde 备份被四个入口消费”；GREEN 后新增 service 与 manager seam 回归覆盖 Home 字节、route、recovery 和 runtime 不变。
- 验证：Home 32/32、RouteManager 90/90、Codex Profile 176/176、Rust lib 2471 passed/2 ignored、前端 Vitest 93 files/637 tests、TypeScript typecheck、renderer production build、Rust 格式与 diff check 全部通过。
