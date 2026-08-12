# 09 — 禁止当前 writer 静默降级为旧备份

Status: resolved

**构建内容：** 修复最终 Standards 审查发现的安全降级：当前 writer 构建 ownership proof 失败时必须传播错误，不得生成伪装成 v1 的弱语义备份。

**受阻于：** 08 — 覆盖 inline token 并保护关闭态同供应商模型。

## Acceptance Criteria

- [x] 当前 writer 构建严格 ownership proof 失败时返回错误且不写 Home/backup/route。
- [x] 当前 writer 成功输出始终为 `CODEX_ROUTE_BACKUP_VERSION`；v1/v2 只允许来自历史读取或显式测试夹具。
- [x] 非 UTF-8、缺严格字段、不可无损表达 token 等错误保持 fail closed，不进入弱 v1 ownership 判定。
- [x] v1/v2 读取兼容与 v3 正常生命周期回归通过。
- [x] Codex Profile、格式、Clippy 归因与相关全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，先以公开服务/manager seam 建立 writer 降级 RED。
- 不删除历史 v1/v2 兼容路径；只禁止当前 writer 新建旧版本。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 的 Home service RED 证明旧 writer 在缺 `base_url` 时会静默输出 `version=1` 且没有 ownership proof；当前 writer 现改为传播 proof、UTF-8 与 listener token 错误，成功结果固定为 v3。
- manager seam 的第一轮 RED 进一步发现 writer 拒绝有损 token 备份后仍残留 `enable_started` recovery；备份预构建现位于 operation 持久化与 runtime 启动之前，失败补偿后 Home、backup、route、runtime 均保持原样。
- 历史 v1 恢复测试改用显式 v1 JSON 夹具，不再借当前生产 writer 制造旧版本；现有 v1/v2 读取迁移与 v3 生命周期保持通过。
- 验证：Home 37/37、RouteManager 92/92、Codex Profile 183/183、TypeScript typecheck、Rust 格式与 diff check 通过。Rust lib 2477 passed/2 ignored，唯一 `model_pricing` 并发文件时序测试失败，隔离 exact 重跑通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件的 8 个既有 lint：migration 1 项、body_dump 6 项、forwarder 1 项；Issue 09 修改文件无报告。
