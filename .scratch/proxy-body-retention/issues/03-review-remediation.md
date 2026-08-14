# 03 — 补齐维护调度回归覆盖并消除重复清理

**Status:** resolved

**构建内容：** 让自动化测试真实驱动应用级 maintenance 调度 seam，证明启动立即执行、周期 tick 继续执行且单次失败不会终止后续循环；同时让历史平面清理测试复用生产树级 seam，避免两套 retention 实现漂移。

**受阻于：** 02 — 接入应用维护生命周期并移除请求清理。

## 覆盖范围

- 补强 `REQ-03`、`REQ-09`、`REQ-10` 的验证证据。
- 补强 `SCN-06`、`SCN-10` 与 `TS-02`，关闭 Spec review 的 P2 finding。
- 清理 Standards review 指出的 `Duplicated Code / Speculative Generality` 判断项。

## Acceptance Criteria

- [ ] 测试驱动 production maintenance 编排 seam，而不是仅手工重复调用 tree cleanup helper。
- [ ] 测试能在“启动立即执行”或“后续周期 tick 被移除”时变红。
- [ ] 单次清理失败后，后续 tick 仍会执行并收敛受控 fixture。
- [ ] 同步文件扫描仍在 blocking task 内执行。
- [ ] 旧平面清理测试复用生产 tree cleanup seam，不再保留第二套 test-only retention 实现。
- [ ] 不删除任何 commented-out code 或与本 review 无关的说明性注释。

## 验证方式

- 按 TDD 记录调度 seam 的 Red→Green；优先使用暂停时间或可控 tick source，保证 deterministic 且以秒计。
- 运行全部 body dump 测试、maintenance 定向测试、Rust 格式/diff 检查和 all-targets check。
- 重新运行受控多目录 repro，并确认请求路径仍无 retention 调用。

## 执行约束

- 修改测试或生产代码前调用 `$tdd`；不要为了测试引入未被 spec 需要的通用调度框架。
- 测试必须对 production 编排接线敏感；只验证 helper 可重复调用不算完成。
- 删除被替代的可执行 helper 允许，但不得删除 commented-out code 或独立说明性备注；如迁移其测试，应保留等价行为覆盖。
- 新函数保持单一职责，新注释使用中文，新常量进入统一 constants 模块。

## 范围之外

- 改变 24 小时间隔、增加锁、容量上限或 UI 配置。
- 修复完整 Rust suite 中范围外的 model_pricing/provider_commands 不稳定测试。

## Comments

- 已由 fresh worker 按 TDD 完成；production `start_periodic_maintenance` seam 统一拥有启动立即执行与 24 小时周期执行。
- Red→Green：测试最初因缺少 production 调度 seam（E0432）和隔离目录 maintenance seam 不可访问（E0603）而失败；最小实现后，暂停 Tokio 时间的真实 maintenance 测试通过。
- 调度测试首次真实运行时发现 Tauri 全局 runtime 不受当前测试的 paused clock 控制；改用项目既有 Tokio runtime 后，虚拟 24 小时 tick 可确定性触发，测试耗时 0.01 秒。
- 旧平面清理测试已改用 production `cleanup_body_dump_tree`，重复的 test-only `cleanup_old_dump_files` 可执行实现已删除，原说明性备注保留。
- 验证：maintenance 定向测试 1/1、body dump 测试 18/18、`cargo fmt --check`、`git diff --check` 与 `cargo check --all-targets` 均通过；未触碰真实日志目录。
- 协调 Agent 独立精确重跑时测试失败：推进 24 小时后 legacy 根层和 inactive Profile 文件均仍存在。interval 在 spawned task 内建立，虚拟时间可能先于其基线初始化，当前测试不是 deterministic；Issue 03 重新打开修复。
- 修复后 interval 在 spawn 前建立并消费 immediate tick，`start_periodic_maintenance` 返回时下一次周期 deadline 已确定；测试改为只在真实 blocking cleanup 返回后发送 completion 信号。
- 修复后的精确测试单次通过，并连续运行 20 次全部通过；body dump 18/18、格式、diff 与 all-targets check 再次通过，Issue 03 重新关闭。
