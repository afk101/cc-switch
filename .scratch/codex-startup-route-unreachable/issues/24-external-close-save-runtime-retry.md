# 24 — External 关闭态保存失败后的 runtime 重试

Status: resolved

**构建内容：** 修复最终认证阻断：External 收敛停止 runtime 后若关闭态 DB 保存失败，不得留下可被后续 switch 当成运行中复用的已停止 runtime。

**受阻于：** 23 — External 切换 PREPARED 前失败收敛。

## Acceptance Criteria

- [x] External 收敛 stop 成功、DB save 失败时，route/Home 保持可重试且 runtime 状态不会伪装为可 swap 的 Running 实例。
- [x] 第二次显式 switch 在临时准备错误消失后成功，必须实际重新创建/启动并通过健康检查，最终 listener=Running、route enabled/Managed。
- [x] 可选实现需保证崩溃一致性：持久化 closing recovery 或在失败后恢复/移除已停 runtime；不得仅依赖内存布尔值。
- [x] stop 失败、DB 重试失败、External 静默关闭和 Managed switch 语义保持。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，用“首次 External 准备失败+关闭保存失败，第二次准备成功”的公开 manager seam 先 RED，并断言真实 runtime Running/health。
- 不写外部 Home，不新增提示或 watcher。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- TDD 公开 manager seam 先稳定 RED：External 的 PREPARED 前失败已成功停止旧 runtime，但关闭态保存失败后，运行时工厂没有创建替代实例；旧实现仍把已停止对象留在 manager 中，下一次 switch 只会对它执行 snapshot swap。
- 收敛保存或故障转移恢复失败时，manager 现在先移除已停止实例，再按数据库仍保留的 enabled/Managed route、provider、failovers 与本地 token 创建、启动并健康检查替代 runtime；只有真实 Running 才重新登记。
- 若替代 runtime 启动、健康检查或登记失败，实例会被停止且不登记；下一次显式 switch 可从“enabled 但无 runtime”按持久态重建后继续，避免假 Running 和永久不可重试。
- 崩溃一致性由持久态保证：关闭保存失败时数据库仍是 enabled/Managed，进程在 stop 后任一窗口退出，下一次正常启动都会依照该权威状态重建 runtime；不依赖仅存在于内存的标志。
- 回归覆盖一次 DB 失败后成功切换、连续 DB 失败逐次替换 stopped runtime、补偿 runtime 首次启动失败后的下一次恢复、stop 失败，以及既有 Managed/External 切换语义。
- 验证：RouteManager 135/135、Codex Profile 212/212、Rust lib 2523 passed/2 ignored、前端排除独立 Node runner 文件后 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check通过。
- `pnpm test:unit` 的 637 个 Vitest 用例均通过，但其额外收集的既有 `scripts/upgrade.test.js` 在当前 Node 环境因 `ERR_INVALID_URL_SCHEME` 失败；该文件与本 issue 无修改。`cargo clippy --lib -- -D warnings` 仍只有未修改文件中的 8 个既有 lint；对四类既有 lint显式 allow 后严格 Clippy 通过。
