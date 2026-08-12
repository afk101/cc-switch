# 21 — 收口 External 切换尾窗原子性

Status: resolved

**构建内容：** 修复 External 显式切换在 failover 阶段补偿与最终 clear 阶段的 ownership 尾窗，确保失败/崩溃始终恢复 External 起点事实。

**受阻于：** 20 — 修正 External 切换与 pending disable 状态判定。

## Acceptance Criteria

- [x] External switch 在推进 `switch_failovers_saved` 保存失败时，补偿使用 `switch_origin_route`，Home 字节不变、ownership=External、backup=None，旧/新 token 不可逆。
- [x] switch pending 各阶段在 recovery 清除前保留起点 ownership；只有最终原子 clear 成功时才转 Managed 并提交新 backup。
- [x] final clear 保存失败后重启恢复，route/Home/backup/ownership 回到 External 起点，不形成 enabled/Managed/无 backup 分裂状态。
- [x] Managed switch 的现有补偿、failovers/runtime/catalog 语义不变。
- [x] Rust、前端、格式、Clippy 归因与全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，两个尾窗都以 External switch 故障注入/重启公开 manager seam 先 RED。
- ownership 转换必须与 recovery 清除、新 backup 提交处于同一最终原子保存。
- 不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 两条公开 manager seam 均先 RED：`switch_failovers_saved` 阶段保存失败补偿与 final clear 保存失败后的 pending route 都错误显示 `Managed`；修复后均稳定 GREEN。
- External 切换的全部 pending route 继承 `switch_origin_route.home_ownership`；故障转移阶段推进保存失败统一用该起点 route 补偿，避免重新带回不可证明旧 backup。
- 最终 clear 的单次 route 保存同时提交 `Managed`、finalized 新 backup、`recovery=None` 与清空错误；clear 失败时数据库仍保留 External 起点 ownership 和可恢复组合 backup，重启恢复回 External/Home 原字节/backup=None。
- 验证：两条新增尾窗测试 2/2、RouteManager 125/125、Codex Profile 202/202、Rust lib 单线程全量 2513 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 全部通过。
- `cargo clippy --lib -- -D warnings` 中本 issue 的 `clone_on_copy` 已清零；允许仓库既有四类 lint 后严格 Clippy 通过。未修改文件仍有 8 个既有 lint：`migration.rs` 1 项、`proxy/body_dump.rs` 6 项、`proxy/forwarder.rs` 1 项。
