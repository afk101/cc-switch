# 04 — 完成旧备份兼容与回归闭环

Status: resolved

**构建内容：** 让升级用户的旧backup、旧占位符和token丢失场景按可证明程度安全迁移，并用完整回归证明修复不会泄漏凭证、覆盖用户配置或破坏其他Profile。

**受阻于：** 03 — 显式路由操作重建最新基线。

## 覆盖范围

- `REQ-13`、`REQ-14`、`REQ-15`、`REQ-16`、`REQ-17`、`REQ-18`及全量回归
- `SCN-15`、`SCN-16`、`SCN-17`、`SCN-18`、`SCN-19`、`SCN-20`、`SCN-21`

## Acceptance Criteria

- [x] v1 backup在当前token、完整legacy triple或旧target证明成立时升级为新版proof。
- [x] v1模糊token、同形状外部token和不完整legacy配置均静默关闭且不覆盖Home。
- [x] token文件丢失且新版proof成立时只升级token，保留非路由字段并启动listener。
- [x] backup、数据库导出、日志和错误均不包含listener token明文。
- [x] 写前并发冲突、Profile隔离、跨平台文件语义和现有生命周期测试保持通过。
- [x] 现场RED replay由新的行为回归覆盖，并完成Rust全量、前端类型检查和生产构建。

## 验证方式

- 运行Codex Profile定向测试与完整Rust测试套件。
- 运行前端typecheck、相关前端测试和生产构建。
- 运行现场日志replay并说明其作为历史RED证据与新行为测试的对应关系。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`。
- 旧整文件target hash仅作为v1一次性迁移证明，不得重新进入新版语义路径。
- `PROXY_MANAGED`成功升级后不得继续作为常规token proof。
- 若全量测试暴露设计遗漏，回到spec约束修正，不得缩小需求规避。

## 范围之外

- 不新增token轮换UI或后台配置监听。
- 不重构全局proxy takeover。

## Comments

- v1 backup 在当前 listener token 与严格路由目标均一致时改为字段级证明并升级；旧 target 指纹仅作 v1 一次性迁移证明。
- 新增公开 manager seam 回归，覆盖 v2 token 文件丢失、v1 可证明升级、模糊/同形状外部 token/不完整 legacy fail-closed，并验证模型、Desktop、plugins 保留与 token 不泄漏。
- 现场日志 replay 仍稳定复现历史 RED：`reconcile_home/derived_state` 后直到手动生命周期操作才出现 listener；新行为由“非路由字段变化仍恢复 runtime”及上述兼容回归对应覆盖。
- 验证：Home 28/28、RouteManager 82/82、Rust lib 2459 passed/2 ignored、前端 typecheck、Vitest 93 files/637 tests、renderer production build 通过。Tauri release 已生成 `.app`、DMG 和 updater tar，最后仅因环境未提供 `TAURI_SIGNING_PRIVATE_KEY` 无法签名而返回失败。
- 既有失败：`provider_commands` 的旧 Codex 全局 switch test hook 与当前“Codex 必须指定 Profile”契约不一致，首项失败后毒化共享 mutex；`pnpm test:unit` 中 `scripts/upgrade.test.js` 使用 Node test runner，被 Vitest 收集时会报 URL scheme/no-suite，排除它后全部前端测试通过。
