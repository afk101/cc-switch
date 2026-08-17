# 03 — 多 Profile 显式 Sync、Import 与云恢复

Status: ready-for-agent

**构建内容：** 手动 Sync、数据库导入与云恢复完成后，逐个 Managed Profile 按自己的主供应商同步 model family；单个失败不阻塞其他 Profile，并向用户返回脱敏的部分成功警告。

**受阻于：** 01 — Model family 权威投影原语。

## 覆盖范围

- Requirements：REQ-09、REQ-11、REQ-12、REQ-13、REQ-15、REQ-16。
- Scenarios：SCN-12、SCN-13、SCN-14、SCN-15、SCN-16、SCN-18。
- 本 slice 贯通 route-aware manager、现有 Sync/Import/Restore 命令结果与用户可见 warning。

## Acceptance Criteria

- [ ] 手动 Sync 遍历所有 Managed Profile，并按各自的有效主供应商同步 model family。
- [ ] Import 与云恢复在 DB 成功后触发同一 route-aware Profile 同步。
- [ ] External 跳过；没有主供应商或单个 Home 失败时记录该 Profile 结果并继续。
- [ ] 已成功 Profile 不因后续失败回滚；Import/Restore 的 DB 成功不回滚。
- [ ] 全成功返回成功；部分失败返回完成状态与逐 Profile 脱敏 warnings。
- [ ] warning 不包含 token、API Key 或配置正文，并能让用户识别需要修复的 Profile。
- [ ] 普通 startup 继续保留 model family；下一次明确 Sync 可以修复上次失败或崩溃残留。
- [ ] 先在 route-aware explicit sync public seam 观察 RED，再完成 GREEN；命令层只测试委托和公开结果。

## 验证方式

- 运行 route-aware explicit sync 的多主供应商、External、缺失主供应商、写失败继续与 warning 测试。
- 运行数据库 Import 和云恢复的全成功、部分失败用户结果测试。
- 运行现有 global Live sync、import/export 与 Profile startup 对账回归。
- 运行前端 typecheck、相关测试、Rust formatter 与静态检查。

## 执行约束

- 修改测试或生产代码前调用 `$tdd`。
- 使用结构化 outcome 表达部分成功，不从日志文本反向解析结果。
- 不将 best-effort 扩散到交互式 provider save。
- 不新增 journal；不让 startup 自动修复 model-family 残留。
- 失败摘要必须在靠近错误源的位置脱敏，并保持现有 Import/Restore DB 成功语义。

## 范围之外

- Provider save 的全引用原子扇出和 provider key rename。

## Comments

- 若现有云恢复入口有多个调用方，所有 post-import 路径必须汇入同一显式同步 contract。
