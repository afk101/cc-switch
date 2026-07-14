# Cargo 测试执行异常 Findings

## 现象

- 执行 Codex 多 Home 实施计划的任务 1 时，先新增了 v11 -> v12 Schema 的 RED 测试，尚未写任何生产代码。
- 目标命令为：`cargo test --manifest-path src-tauri/Cargo.toml migrates_v11_to_v12_codex_profile_schema --lib`。
- 原 target 下三次执行均在完整依赖冷编译阶段受到运行器会话中断或输出脱离影响，未生成包含新测试的测试二进制。

## 已收集证据

- 原 target 曾持有 `src-tauri/target/debug/.cargo-lock`；诊断期间没有并发启动 Cargo。
- 原生依赖 `aws-lc-sys`、`ring`、`zstd-sys`、`lzma-sys` 一度持续生成目标文件，说明首次现象不是立即的 Rust 测试失败。
- 已尝试不同的隔离方案：唯一临时目录 `/tmp/cc-switch-codex-profile-tdd.VZtAsh`、独立 `CARGO_TARGET_DIR`、系统 `clang/clang++`。
- 后台隔离方案被运行器在父 shell 退出时清理，Cargo 尚未实际启动；该路径不再重复。
- 前台 PTY 隔离构建获得真实依赖错误，尚未进入测试：

```text
failed to run custom build command for `proc-macro2 v1.0.106`
could not execute process .../build-script-build (never executed)
No such file or directory (os error 2)
```

- `serde_core`、`quote` 出现相同错误。这表明隔离 target 内刚构建的 macOS 可执行 build script 无法被执行，当前还不能证明 v12 schema 测试的 RED 行为。
- 当前工作树唯一代码变更是 `src-tauri/src/database/tests.rs` 中的 RED 测试；没有生产代码、DAO、schema 或 Home 文件改动。

## 已排除的方向

- 未出现并发 Cargo 编译导致的持续锁竞争。
- 未执行任何 schema 生产实现，因此错误不能由 v12 实现引起。
- 后台 shell 方案无法在该运行器中保留子进程，不能作为有效测试执行机制。
- 只读环境诊断确认：主机为原生 `arm64`，`rustc` host 为 `aarch64-apple-darwin`，未运行 Rosetta；Xcode clang、macOS SDK 与默认 Rust target 都可用。
- 未发现 `CARGO_BUILD_TARGET`、`RUSTFLAGS`、`CC`、`CXX`、`SDKROOT`、`ARCHFLAGS` 或 `DYLD_*` 覆盖；`/tmp` 与工作树同在可执行的 APFS Data 卷，未见 `noexec`。
- 持久 `src-tauri/target` 中同 hash 的 `proc-macro2` build script 是 `Mach-O 64-bit executable arm64`，权限为可执行、仅依赖 `/usr/lib/libSystem.B.dylib`，并可被系统正常加载（因手动缺少 Cargo 的 `RUSTC` 环境变量而退出）。因此架构、动态加载器与卷执行权限不是根因。
- 隔离 target 内不存在任何可执行文件；`proc-macro2`、`serde_core`、`quote` 都只留下 `.d` 文件与 `invoked.timestamp`，对应的 `build-script-build` 文件缺失。这与 Cargo 报告的 `No such file or directory` 一致。

## 当前假设与最小验证

- 假设：隔离 target 的可执行 Cargo 产物在编译后、Cargo 执行前未被保留或被外部清理；项目 Rust 代码、架构、SDK 与挂载权限不是触发条件。
- 最小验证：不再使用隔离 target，不改任何代码，在前台 PTY 中以默认的持久 `src-tauri/target` 执行同一个 RED 测试。该 target 已存在可加载的 build script；若测试能进入 v12 断言并按预期失败，假设成立且任务可继续 TDD。

## 默认 target 验证结果

- 使用默认 `src-tauri/target`、未设置 `CARGO_TARGET_DIR`/`CC`/`CXX`，在前台 PTY 中执行同一条测试命令后，Cargo 在 1 分 51 秒内完成并以退出码 0 运行测试二进制。
- 未再次出现 `build-script-build` 缺失或 `os error 2`，因此已确认隔离 target 的可执行产物缺失是此前环境阻塞的直接原因。
- 该次输出为 `running 0 tests`、`1745 filtered out`：新加的 `migrates_v11_to_v12_codex_profile_schema` 没有被测试二进制发现，尚未得到 TDD 所需的 schema RED 失败。
- 下一项独立调查：定位该测试为什么未注册；在测试被发现并按“v12 未实现”失败前，仍不写生产代码。

## 测试发现根因

- `src-tauri/src/lib.rs` 无条件声明 `mod database;`，`src-tauri/src/database/mod.rs` 以 `#[cfg(test)] mod tests;` 纳入 `tests.rs`；新增测试位于普通顶层 `#[test]` 上下文，没有 feature 或 cfg 排除条件。
- 直接读取实际运行的测试二进制 `src-tauri/target/debug/deps/cc_switch_lib-9774ce0f2a07357c --list`，其中有 14 个既有 `database::tests::*`，但没有新增的 `database::tests::migrates_v11_to_v12_codex_profile_schema`。
- `tests.rs` 修改时间为 2026-07-13 17:56:54；实际测试二进制却为 2026-07-08 12:03:22。相反，dep-info 与 test fingerprint 已更新到 2026-07-13 并明确记录了 `src/database/tests.rs`。
- 因此根因已确认：默认 target 的 Cargo 元数据/dep-info 与实际 test harness 不一致，Cargo 执行了陈旧二进制，才出现 `running 0 tests` / `1745 filtered out`。这不是新增测试的 module、feature 或名称问题。

## 建议的最小恢复动作

- 在不改代码的前提下执行 `cargo clean -p cc-switch`，只清理该 package 的本地 Cargo 产物与 fingerprint，不清空整个 target。
- 随后用默认 target、前台 PTY 重新执行同一条 RED 测试，并先用生成的 `cc_switch_lib-* --list` 确认新测试出现。只有得到“v12 尚未实现”的正确 RED 后，任务 1 才能恢复生产实现。

## 已确认的恢复决策

| 决策 | 理由 |
|------|------|
| 执行 package 级 `cargo clean -p cc-switch`，随后在默认 target 重跑 RED 测试 | 用户已确认。该操作只清理已被证实不一致的 package 构建产物，影响面小于全量 target 清理，并直接验证“陈旧 harness”假设。 |

## package 级清理后的新证据

- 已确认没有并发 `cargo`/`rustc` 后，执行 `cargo clean --manifest-path src-tauri/Cargo.toml -p cc-switch` 成功，输出为 `Removed 2593 files, 3.1GiB total`。
- 随后以默认 target 的前台 PTY 重新执行目标 RED 测试，Cargo 以退出码 101 失败于本包 build script：

```text
failed to run custom build command for `cc-switch v3.16.5`
could not execute process `.../src-tauri/target/debug/build/cc-switch-ac5a4c6cedb8651a/build-script-build` (never executed)
No such file or directory (os error 2)
```

- 清理前能运行测试只因复用了旧 harness；清理后连新编译的 `cc-switch` 自身 build script 都无法被 Cargo 执行。因此“仅隔离 target 产物不一致”不是完整根因，当前更强的假设是：此环境在 Cargo 编译后、执行前未保留或拦截新生成的 Rust build-script。
- 未进入测试阶段，新增 RED 测试仍未验证；没有生产代码改动。

## build script 生命周期监测

- 因 `fs_usage -w -f filesystem` 需要 root 且系统无 `fswatch`，使用只读 `find + stat` 每 200ms 监测 `src-tauri/target/debug/build/cc-switch-*`，持续约 111 秒、共 1176 行快照；监测进程已停止且没有残留 Cargo/Rustc。
- 本次 Cargo 只启动一次，仍以退出码 101 报 `cc-switch-ac5a4c6cedb8651a/build-script-build` 不存在。
- 构建期间 `.d` 文件在 `1783939287` 更新、`invoked.timestamp` 在 `1783939288` 更新；整个轮询中 `build-script-build` 匹配次数为 0，最终也不存在。
- 这进一步排除“测试过滤问题”和“build script 已稳定落盘但不能执行”。当前证据表明 Cargo 的 build-script 可执行产物没有被稳定保留；200ms 采样仍无法绝对排除它在两个采样点之间被外部进程立即删除。

## 待确认的第一手信息

- 需要用户确认：请在普通 macOS Terminal（不要通过 Codex 内置终端）中运行一次 `cd /Users/qihoo/Documents/A_Code/Fork/cc-switch && cargo test --manifest-path src-tauri/Cargo.toml migrates_v11_to_v12_codex_profile_schema --lib`。若能成功进入测试或出现相同 build-script 错误，可区分是 Codex 运行环境限制还是机器级 Rust/安全软件问题。

## 普通 Terminal 复现与 linker 配置线索

- 用户在普通 macOS Terminal 从仓库根执行同一命令后，完整复现 `cc-switch ... build-script-build (never executed): No such file or directory`；因此已排除 Codex 内置终端特有的进程环境。
- `src-tauri/.cargo/config.toml` 的现有中文注释已明确说明：PATH 中 nvm 的同名 `cc`（Node 脚本）会遮蔽系统 clang driver，并导致“编译出的 build script 无法执行（No such file or directory / os error 2）”；该文件将 linker 与 `CC/CXX` 固定为 `/usr/bin/cc`、`/usr/bin/c++`。
- 当前从仓库根的 shell 验证：`command -v cc` 为 `/Users/qihoo/.nvm/versions/node/v22.22.0/bin/cc`，`file` 确认它是 `/usr/bin/env node` 脚本文本；`which -a cc` 中真正的 `/usr/bin/cc` 位于其后。
- 仓库根不存在 `.cargo`，配置仅在 `src-tauri/.cargo/config.toml`。当前假设：从根目录以 `--manifest-path src-tauri/Cargo.toml` 运行时未发现子目录配置，Rust 回退到被遮蔽的 `cc`；需要用官方 Cargo 配置发现规则和“从 `src-tauri` 作为 cwd 运行”的最小对照确认。

## 检索问题

| 问题 | 调整方案 |
|------|---------|
| Exa 查询官方 Cargo 文档时响应没有 `results` 数组，`jq` 无法迭代 | 不重复相同 API 请求，改直接读取 `doc.rust-lang.org` 官方 Cargo 配置参考页。 |

## 待确认的第一手信息

- 用户反馈：这台机器/这个 worktree 过去可能没有成功运行过 `cargo test --manifest-path src-tauri/Cargo.toml ...`。因此暂不把本次未提交的 RED 测试视为回归，优先调查本机构建工具链、架构与可执行文件环境。

## 当前状态

- 根本原因尚未确认；停止实现，等待第一手环境信息后继续根因调查。

## 根本原因确认（最终）

- 官方 Cargo 配置参考页的“Hierarchical structure”明确说明：Cargo 从**当前工作目录**及其父目录向上查找配置文件。
- 因此，在仓库根运行 `cargo test --manifest-path src-tauri/Cargo.toml ...` 时，Cargo 不会向下发现 `src-tauri/.cargo/config.toml`；`--manifest-path` 不改变配置搜索起点。
- 未加载该配置时，Rust 通过 PATH 解析 `cc`，实际命中 nvm 的 Node 脚本而非 `/usr/bin/cc`。这与 `src-tauri/.cargo/config.toml` 中已记录的已知症状逐字吻合：链接阶段异常会使新生成的 Rust build script 以 `No such file or directory` 无法执行。
- 根本原因已确认：**从错误工作目录运行 Cargo，导致 `src-tauri/.cargo/config.toml` 未被加载，nvm 的同名 Node `cc` 遮蔽系统 clang linker。**
- 最小恢复方式：从 `src-tauri` 目录运行 `cargo test migrates_v11_to_v12_codex_profile_schema --lib`，使 Cargo 读取现有配置；不需要修改产品代码或 Cargo 配置。

## 正确 RED 已观察到

- 用户从 `src-tauri` 目录运行 `cargo test migrates_v11_to_v12_codex_profile_schema --lib` 后，Cargo 在 1 分 32 秒完成构建并执行了 1 个目标测试。
- 测试按预期失败：`src/database/tests.rs:440` 断言当前 user_version 为 11，而要求 v12。输出为 `0 passed; 1 failed; 1745 filtered out`。
- 这证明 `src-tauri/.cargo/config.toml` 已被加载、测试被正确发现，失败原因是任务 1 尚未实现 v11 -> v12 schema migration；可以恢复 TDD 的 GREEN 实现。

## 调查过程修正

| 问题 | 调整方案 |
|------|---------|
| 追加根因段落时使用的补丁上下文已被前序 findings 追加改变，补丁未应用 | 改为读取文件尾部后按实际位置追加，未重复原补丁。 |
