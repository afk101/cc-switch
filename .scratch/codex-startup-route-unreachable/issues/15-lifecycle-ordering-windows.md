# 15 — 关闭重分类与启用写前基线

Status: resolved

**构建内容：** 修复终审发现的两个生命周期时序窗口：关闭预检后发生外部编辑时可重新分类收敛；显式启用必须在任何 Home 投影写入前保存真实原始基线与 recovery。

**受阻于：** 14 — 显式关闭前重新识别外部接管。

## Acceptance Criteria

- [x] Managed 关闭预检后、Home restore 写入前发生严格字段外部编辑时，CAS/所有权冲突触发只读重分类。
- [x] 重分类确认为 External 后保持 Home、停止 runtime、清 stale backup/recovery、提交 disabled/External；瞬态 I/O 保留可重试 recovery。
- [x] 已落 `disable/home_restore_failed` 的历史/崩溃 recovery 在恢复时同样可重分类 External，不再永久卡死。
- [x] 显式启用在任何 model catalog、MCP 或 route 写 Home 前，从原始 Home（含“文件不存在”）构建并持久化 backup/recovery。
- [x] 缺失 `config.toml` + provider catalog 的 enable→disable 恢复为文件不存在；写入各阶段崩溃可由 recovery 补偿。
- [x] 既有模型目录投影、正常启停、失败补偿、Profile 隔离与脱敏语义保持不变。
- [x] Rust、前端、格式、Clippy 归因与相关全量回归通过。

## 执行约束

- 修改前调用 `$tdd`，两个真实 manager seam 均先 RED。
- 写 Home 前必须已有可恢复持久化记录；不得用投影后的内容冒充用户基线。
- 不增加 watcher，不修改用户无关 `.scratch/wiscode-codex-x-api-key-401/`。

## Comments

- 关闭路径在字段恢复失败后复用同一只读所有权预检：若严格路径已被外部接管，立即停接并以单条 route 保存收敛为 disabled/External；若二次读取为瞬态 I/O 或 Home 仍受管，则保留 enabled/Managed、backup 与 recovery 供后续重试。
- 历史 `disable_home_restore_failed` recovery 使用相同重分类分支，不再对已外部接管的 Home 永久重放陈旧 restore。
- 显式启用从同一份原始 Home snapshot 组合构造模型目录投影与最终路由目标，先序列化并持久化原始 backup/recovery，再允许任何模型目录或 `config.toml` 写入；模型目录已写而 route 写失败时会即时补偿，并保留 recovery 继续收尾。
- 原始 `config.toml` 不存在时，backup 明确记录缺失基线；关闭会清理由 CC Switch 注入的模型目录指针与严格路由表，恢复为文件不存在，同时仍保留运行期间新增的其他非路由字段。
- TDD RED 均由公开 manager seam 复现；GREEN 验证 RouteManager 111/111、Codex Profile 203/203、Rust lib 2498 passed/2 ignored、前端 93 files/637 tests、TypeScript typecheck、renderer production build、Rust fmt 与 diff check 通过。
- `cargo clippy --lib -- -D warnings` 本 issue 修改代码无报告；仅剩未修改文件中的 8 个既有 lint：migration 1 项、body_dump 6 项、forwarder 1 项。
