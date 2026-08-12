# 04 — 完成旧备份兼容与回归闭环

Status: ready-for-agent

**构建内容：** 让升级用户的旧backup、旧占位符和token丢失场景按可证明程度安全迁移，并用完整回归证明修复不会泄漏凭证、覆盖用户配置或破坏其他Profile。

**受阻于：** 03 — 显式路由操作重建最新基线。

## 覆盖范围

- `REQ-13`、`REQ-14`、`REQ-15`、`REQ-16`、`REQ-17`、`REQ-18`及全量回归
- `SCN-15`、`SCN-16`、`SCN-17`、`SCN-18`、`SCN-19`、`SCN-20`、`SCN-21`

## Acceptance Criteria

- [ ] v1 backup在当前token、完整legacy triple或旧target证明成立时升级为新版proof。
- [ ] v1模糊token、同形状外部token和不完整legacy配置均静默关闭且不覆盖Home。
- [ ] token文件丢失且新版proof成立时只升级token，保留非路由字段并启动listener。
- [ ] backup、数据库导出、日志和错误均不包含listener token明文。
- [ ] 写前并发冲突、Profile隔离、跨平台文件语义和现有生命周期测试保持通过。
- [ ] 现场RED replay由新的行为回归覆盖，并完成Rust全量、前端类型检查和生产构建。

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
