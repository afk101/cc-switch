# 02 — 启动恢复安全收敛外部接管

Status: ready-for-agent

**构建内容：** 应用启动时先验证 enabled Profile 的有效路由所有权；正常非路由变化继续启动服务，真实外部接管则保护Home并静默关闭，临时I/O错误保持可重试状态。

**受阻于：** 01 — 建立字段级路由所有权与版本化证明。

## 覆盖范围

- `REQ-01`、`REQ-02`、`REQ-03`、`REQ-04`、`REQ-05`、`REQ-06`、`REQ-07`、`REQ-08`、`REQ-09`、`REQ-16`、`REQ-17`、`REQ-18`
- `SCN-01`、`SCN-02`、`SCN-03`、`SCN-04`、`SCN-05`、`SCN-06`、`SCN-07`、`SCN-08`、`SCN-09`、`SCN-10`、`SCN-21`

## Acceptance Criteria

- [ ] 正常模型及非路由字段变化后，真实启动序列成功创建一个listener且Home相关字段不被覆盖。
- [ ] selector或严格字段外部修改、配置缺失/损坏时，Home原样、runtime不启动、route静默关闭并进入稳定外部接管状态。
- [ ] 外部接管状态保留主/故障转移供应商引用、清旧backup，并阻止后续关闭态派生对账写Home。
- [ ] 临时I/O与DB保存失败不会销毁enabled、backup或ownership状态，也不会启动runtime。
- [ ] 所有权检测先于enabled Home派生写入；单Profile失败不影响其他Profile。
- [ ] UI可观察route开关为关闭且没有持久提示，应用日志保持脱敏。

## 验证方式

- 运行 `CodexRouteManager` 真实两阶段启动顺序的定向测试。
- 运行route DAO/schema迁移测试及前端route状态类型检查。
- 以文件字节、route状态和runtime创建次数证明每个分支。

## 执行约束

- 修改生产代码或测试前调用 `$tdd`。
- 自动外部接管不得复用会恢复Home的正常关闭流程，也不得伪造disable recovery。
- 外部接管持久状态不得使用错误文案作为控制标记。
- 不触碰auth.json、token store或其他Profile Home。

## 范围之外

- 不增加后台watcher、轮询或UI提示。
- 不完成显式切换供应商的新基线行为。
