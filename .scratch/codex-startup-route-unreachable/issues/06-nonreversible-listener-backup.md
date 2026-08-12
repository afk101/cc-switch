# 06 — 消除备份中可逆 listener token

Status: resolved

**构建内容：** 修复最终 Spec 复审发现的可逆凭证持久化：`previous_content` 即使以 JSON 数字数组保存，也不得包含可还原的 CC Switch listener token。

**受阻于：** 05 — 修正审查发现的基线泄漏与 External 覆盖。

## 覆盖范围

- `REQ-11`、`REQ-14`
- `SCN-11` 重新接管与关闭恢复

## Acceptance Criteria

- [x] 外部接管仅修改 `base_url`、保留原 listener token 后，显式重新启用产生的整份 backup 解码后仍无法找到/还原 listener token。
- [x] backup 用明确的不可逆字段状态或安全 sentinel 表达“接管前 token 是该 Profile listener token”，不得伪装成普通明文配置。
- [x] 后续关闭能通过私有 secret store 恢复该受管 token 前态，同时恢复用户最新 `base_url` 并保留非路由字段。
- [x] token 丢失/重建、失败补偿、旧 backup 兼容和 Profile 隔离保持 fail-closed，不把 token 写入 DB、日志或错误。
- [x] Codex Profile、数据库、前端、格式与相关全量回归通过。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`；先写会解码 backup bytes 的 RED，不得只做 JSON 字符串 contains。
- 不保存 listener token 的明文、可逆编码或加密副本；只允许域分离摘要及不可逆引用语义。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 路由备份升级为 v3；当接管前字段等于该 Profile listener token 时，`previous_content` 删除该字段，并以 `previous_token_state=listener_token_reference` 表达不可逆引用语义。
- 显式关闭与启用崩溃补偿仅从 Profile 私有 token store 解析该引用；私有 token 缺失时保留 Home 和 recovery 状态，不调用 `ensure_token`、不猜测或重建凭证。
- v1/v2 备份保持可读；v2 在下次安全重定位时升级为 v3 并消除可逆 listener token，未知未来版本保守拒绝。
- 验证：Home 29/29、RouteManager 87/87、Codex Profile 170/170、数据库 24/24、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust 格式与 diff check 通过。
- Rust lib 全量 2464 passed/2 ignored；唯一 `model_pricing` 并发文件时序测试失败，隔离 exact 重跑通过，与本 issue 改动无关。
