# 11 — 在 token 创建前完成外部接管预检

Status: resolved

**构建内容：** 修复最终 Spec 复审的两个组合边界：旧 v3 无路径引用与 secret 丢失时用已证明的旧 Home token解析来源；外部接管必须在 `ensure_token` 前被识别，避免创建无用凭证。

**受阻于：** 10 — 保留 token 原始路径并清理健康启动诊断。

## Acceptance Criteria

- [x] 旧 v3 pathless backup、当前 Home 仍含可由 backup proof 证明的旧 listener token、secret 丢失时，使用旧 Home token 唯一解析 origin，再安全创建/写入新 token 与新版 proof并启动 listener。
- [x] 无 proof 或来源不唯一时 fail closed，不凭 token 形状猜测，不覆盖 Home。
- [x] 严格字段已外部接管且 secret 缺失时，启动预检先静默关闭为 External，不调用 `ensure_token`、不创建凭证、不启动 runtime。
- [x] 受管 Current、v1/v2/legacy、自愈 token 与正常首次启用行为保持兼容。
- [x] Home、secret、route、runtime、Profile 隔离及脱敏回归通过。
- [x] Rust、前端、格式、Clippy 归因与相关全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，两个组合场景都用公开 manager seam 先 RED。
- 分类前不得产生 token、Home、runtime 副作用；只有可证明受管的修复分支可创建新 token。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 的两个核心 manager seam 先稳定 RED：旧 v3 无路径引用在 `ensure_token` 后失去旧 token 来源证明；严格字段外部接管在分类前错误创建凭证。最小实现后两者转 GREEN，并补充无唯一来源证明时零凭证副作用的 fail-closed 回归。
- 启动准备现在先调用只读 `read_token`。secret 缺失时由 Home service 仅依据当前 Home、受支持 backup、公开路由目标及 token 摘要证明所有权；旧 v3 无路径引用先用 Home 中旧 token 补全不可逆来源，只有 runtime 恢复阶段才创建新 token。
- 第一阶段派生对账以只读模式调用同一预检，已证明受管但缺 secret 时不创建凭证；外部接管在任何 `ensure_token` 前收敛为 External/disabled，Home、secret 与 runtime 保持无副作用。
- Current、v1/v2、严格 legacy、首次启用、Profile 隔离与既有 token 自愈回归保持通过；新备份中不包含旧 token 或新 token 明文，非路由 model/Desktop 字段保持不变。
- 验证：两条核心组合 seam 与模糊来源 fail-closed 均通过；RouteManager 96/96、Codex Profile 172/172、Rust lib 2483 passed/2 ignored、前端 TypeScript typecheck、Vitest 93 files/637 tests、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中本 issue 修改文件无报告；仅剩未修改文件的 migration 1 项、body_dump 6 项、forwarder 1 项，共 8 项既有 lint。
