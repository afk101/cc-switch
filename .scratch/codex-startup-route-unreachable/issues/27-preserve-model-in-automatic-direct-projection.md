# 27 — 自动直连投影保留用户模型

Status: resolved

**构建内容：** 修复封口 Spec 剩余缺口：disabled/Managed Profile 的启动对账与共享供应商保存属于自动投影，不得覆盖用户自选 `model`；只有显式切到不同供应商才可应用目标模型。

**受阻于：** 26 — 恢复测试夹具统一 operation 常量。

## Acceptance Criteria

- [x] disabled/Managed Profile 启动派生对账保留当前 Home 的 `model`、reasoning 与非路由字段，同时更新必要直连/provider/MCP 派生字段。
- [x] 共享供应商保存对 disabled/Managed Home 自动投影时保留用户 `model`；其他 provider 字段按既有事务语义更新。
- [x] 同供应商显式选择继续保留 model；显式切到不同供应商仍允许应用目标默认 model。
- [x] External Home 仍不参与自动投影；enabled listener 启动仍只改严格路由字段。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，启动与共享供应商保存两个公开 manager seam 先 RED。
- model 授权只来自显式 different-provider 操作，不从自动对账推断。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 两个公开 manager seam 均先稳定 RED：disabled/Managed 启动派生对账与共享供应商保存都会把用户模型改回供应商默认值，并删除 reasoning、Desktop 或未知字段；GREEN 后两条路径都改用自动直连投影。
- 直连计划使用显式 `ApplyProviderModel` / `PreserveUserModel` 模式区分授权来源；显式切到不同供应商继续使用前者，启动对账和共享供应商保存只使用后者，不通过写后恢复掩盖覆盖。
- 自动投影以当前 Home 为基线递归合并供应商明确声明的字段，MCP 表按数据库权威版本替换；顶层 `model` 与 `model_reasoning_*` 永远保留，目标未声明的 Desktop、插件和未知字段保持不变，模型目录继续更新。
- 事务与状态回归通过：共享供应商 Home 写失败和数据库提交失败均恢复完整批次；同供应商选择保留模型、不同供应商显式切换应用目标模型、External 跳过、enabled 严格路由恢复保持原语义。
- 验证：RouteManager 137/137、Codex Profile 230/230、Rust lib 单线程全量 2525 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 仅报告未修改文件中的 8 个既有 lint；显式允许这四类既有 lint 后严格 Clippy 通过，Issue 27 修改文件无告警。
