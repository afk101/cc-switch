# 02 — 接入应用维护生命周期并移除请求清理

**构建内容：** 让应用启动与现有每日 maintenance 统一执行全局 body dump retention，即使 dump 未启用也能自动迁移历史日志，同时让代理请求不再承担目录扫描与删除。

**受阻于：** 01 — 建立全局树级按日清理。

## 覆盖范围

- 覆盖 `REQ-03`、`REQ-04`、`REQ-05`、`REQ-09`、`REQ-10`。
- 覆盖 `SCN-06`、`SCN-09`、`SCN-10`、`SCN-11`。
- 交付 `TS-02`、`TS-03`，完成历史迁移、运行时调度和原始故障闭环。

## Acceptance Criteria

- [ ] 应用启动立即安排一次全局清理，持续运行时每 24 小时再次安排。
- [ ] 清理不受 body dump 编译期开关影响，旧版本遗留文件会在首次启动收敛。
- [ ] 同步文件扫描在 blocking task 中执行，不阻塞 async runtime。
- [ ] 清理失败只产生有界摘要，且不会阻断启动、maintenance 后续 tick 或代理请求。
- [ ] 请求级 BodyDumper 创建不再扫描或删除历史日志，并发请求不再产生重复清理告警。
- [ ] 受控的原始多目录 repro 从红转绿，并完成 Stage 6 instrumentation/临时产物清理检查。

## 验证方式

- 运行 maintenance 编排与 BodyDumper 创建的定向测试。
- 运行全部 body dump 测试以及相关 Rust 测试套件。
- 运行 Rust 格式检查、项目类型检查和可行的完整构建/测试命令。
- 搜索并确认不存在本任务临时 `[DEBUG-...]` instrumentation；重新运行受控 feedback loop，确认旧根层和不活跃 Profile 均收敛。

## 执行约束

- 修改测试或生产代码前调用 `$tdd`，按已确认 seams 执行 red → green。
- maintenance 是 retention 的唯一 owner；请求路径不得保留兜底扫描。
- 启动立即清理与 24 小时周期必须复用单一编排逻辑，并保持 best-effort。
- 不直接用测试删除用户真实日志目录；运行时迁移由修复版本正常启动触发。
- 新增函数保持单一职责，新增注释使用中文，不删除任何已有注释或 commented-out code。

## 范围之外

- 新增后台调度框架、跨进程锁或手动清理 UI。
- 容量上限、压缩归档和非标准文件迁移。

## Comments

