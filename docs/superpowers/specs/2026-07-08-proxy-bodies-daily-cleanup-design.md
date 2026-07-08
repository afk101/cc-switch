# Proxy Bodies Daily Cleanup Design

## 背景

`CC_SWITCH_DUMP_BODY=1` 开启后，`src-tauri/src/proxy/body_dump.rs` 会为每个代理请求写入一个独立 dump 文件到 `<log_dir>/proxy-bodies`。前序系统化调试已确认该目录目前没有任何清理、轮转或大小上限逻辑，导致调试期间短时间内快速膨胀。

本设计只解决用户明确要求的“一天一清”：当 body dump 功能开启并即将写入新 dump 时，清理早于当天的 dump 文件。不开启 `CC_SWITCH_DUMP_BODY` 时不触发任何清理逻辑。

## 目标

- 给 `proxy-bodies` 增加“一天一清”策略。
- 清理逻辑仅在 `CC_SWITCH_DUMP_BODY` 开启后随 body dump 写入路径触发。
- 保留当天 dump 文件，删除早于当天的历史 `.log` 文件。
- 清理失败不能影响代理请求主流程，只记录警告日志。
- 通过单元测试覆盖日期判断、文件过滤、删除失败容错边界。

## 非目标

- 不增加后台定时任务。
- 不改主日志 `cc-switch.log` 的轮转逻辑。
- 不引入当天大小上限或文件数量上限。
- 不清理非 `.log` 文件，避免误删用户手动放入的辅助材料。
- 不改变 dump 文件内容、脱敏规则或 SSE 采样策略。

## 推荐方案

在 `BodyDumper::try_new_inner()` 中，获得 `dump_dir()` 和当前本地日期后，先执行 `cleanup_old_dump_files(&dir, today)`，再创建本次 dump 文件。

由于 `try_new_inner()` 只会在 `try_new()` 通过 `is_enabled()` 判断后调用，因此该清理天然只在 `CC_SWITCH_DUMP_BODY` 开启时发生。不开启 dump 时 `try_new()` 直接返回 `None`，不会创建目录，也不会扫描文件系统。

## 清理规则

- dump 文件命名格式继续使用现有规则：`YYYYMMDD-HHMMSS-<request_id>.log`。
- 清理函数只处理扩展名为 `.log` 且文件名能解析出 8 位日期前缀的文件。
- 如果日期前缀 `< today.format("%Y%m%d")`，删除该文件。
- 日期等于今天或晚于今天的文件保留。
- 文件名不符合格式、非 `.log`、目录项读取失败、删除失败时均不 panic；删除失败写 `log::warn!`。

## 组件边界

### `BodyDumper::try_new_inner`

职责：维持现有“创建单请求 dump 文件”的入口，只增加调用清理函数的编排逻辑。它不承担文件名解析细节。

### `cleanup_old_dump_files`

职责：扫描指定目录，删除早于传入日期的历史 dump 日志。它只处理一件事：按日期保留策略删除旧文件。

### `dump_file_date_key`

职责：从文件名中提取可比较的 `YYYYMMDD` 日期键。无法解析时返回 `None`。

## 错误处理

- `dump_dir()` 创建目录失败仍按现有行为返回错误，使 `BodyDumper::try_new()` 静默返回 `None`。
- 清理目录读取失败：记录警告并返回，不影响本次 dump 创建。
- 单个旧文件删除失败：记录警告并继续处理其它文件。
- 文件名解析失败：跳过，不记录噪音日志。

## 测试要求

在 `src-tauri/src/proxy/body_dump.rs` 的现有测试模块中新增单元测试：

1. 删除早于今天的 `.log` 文件。
2. 保留今天的 `.log` 文件。
3. 保留晚于今天的 `.log` 文件，避免系统时间回拨或跨时区异常误删。
4. 跳过非 `.log` 文件。
5. 跳过不符合 `YYYYMMDD-*` 格式的 `.log` 文件。

## 验收标准

- `CC_SWITCH_DUMP_BODY` 未开启时，`BodyDumper::try_new()` 仍直接返回 `None`，不会触发清理。
- `CC_SWITCH_DUMP_BODY` 开启并创建新 dump 时，早于当天的 `proxy-bodies/*.log` 被删除。
- 当天 dump 文件被保留。
- 非 dump 文件和不符合命名格式的文件不被删除。
- 清理失败不阻断代理请求。
- `pnpm run typecheck` 通过。
- `pnpm run build` 通过。
