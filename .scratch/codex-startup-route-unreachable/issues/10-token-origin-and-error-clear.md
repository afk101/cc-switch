# 10 — 保留 token 原始路径并清理健康启动诊断

Status: resolved

**构建内容：** 修复 release Spec 复审的两个生命周期缺口：v3 listener 引用必须记录接管前 token 的准确原始路径；健康启动必须清除上一轮启动错误，即使 backup 无需 rebase。

**受阻于：** 09 — 禁止当前 writer 静默降级为旧备份。

## Acceptance Criteria

- [x] 顶层 fallback listener token 被接管后，v3 backup 以不可逆且带路径的引用记录其原始位置。
- [x] 关闭/补偿恢复时删除路由目标 provider token，并把 listener token 只恢复到接管前顶层位置；不得残留或错发。
- [x] provider 内、顶层、inline/标准 table 及旧 v3 无路径引用兼容保持 fail closed/可证明迁移。
- [x] 本次 startup runtime 健康成功后 route `last_error` 必须为 `None`，不依赖 backup 是否变化。
- [x] 启动失败诊断仍脱敏保留，健康 Profile/其他 Profile 隔离不变。
- [x] Codex Profile、数据库、前端、格式、Clippy 归因与全量相关回归通过。

## 执行约束

- 修改前调用 `$tdd`，顶层 fallback 完整 enable→disable seam 与 stale error→健康 startup seam 均先 RED。
- listener token 仍不得以明文、可逆编码或加密副本进入 backup。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- v3 当前 writer 新增不可逆 `previous_token_origin`：顶层编码为 `top_level`，provider 标准表与 inline table 统一编码 provider ID 路径；listener token 正文仍只存在于 Profile 私有 secret store。
- 完整 enable→disable manager seam 先 RED 证明旧引用会把顶层 fallback token 错恢复到目标 provider；GREEN 后关闭先清理目标 provider token，再只把私有 token 恢复到接管前顶层位置，并逐字节恢复 Home。
- 历史 pathless v3 不猜路径：用已脱敏正文分别重建顶层与活动 provider 候选，只有接管前整文件指纹唯一成立时才消费；证明不成立时保持 Home 不变并返回脱敏错误。新 writer 的 provider 标准表、inline table 与顶层路径回归均通过。
- Current Home 且 backup 无需 rebase 的真实启动 seam 先 RED 证明健康 listener 启动后仍残留旧 `last_error`；GREEN 后在 runtime 健康成功时独立清错，清错持久化失败会停止刚启动的 runtime 并进入既有脱敏失败诊断，健康检查与 runtime 登记失败诊断保持通过。
- 验证：Home 38/38、RouteManager 93/93、Rust lib 2480 passed/2 ignored、TypeScript typecheck、前端 Vitest 93 files/637 tests（排除既有 Node runner `scripts/upgrade.test.js`）、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中本 issue 新增代码无报告；仅剩未修改文件的 migration 1 项、body_dump 6 项、forwarder 1 项，共 8 项既有 lint。
