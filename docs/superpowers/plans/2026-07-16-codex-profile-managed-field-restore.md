# Codex Profile Managed-Field Restore Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Codex Profile 关闭路由时只验证并恢复三个路由字段，同时保留 Codex Desktop 和用户对其他配置的最新修改。

**Architecture:** 在 `CodexHomeConfigService` 内实现以 previous/target/current 为输入的 TOML 字段级三方恢复，三个字段的路径分别从现有路由构造器生成的 target 推导。`CodexRouteManager` 只读取现有 listener token 并复用统一恢复入口；现有 backup/recovery 格式保持不变。

**Tech Stack:** Rust、Tauri、`toml_edit`、Serde、Tokio、Rusqlite、Cargo test

---

## 文件结构

- Modify: `src-tauri/src/codex_profile/constants.rs` — 三个字段名、固定 wire API 值和脱敏错误标签。
- Modify: `src-tauri/src/codex_profile/home_config.rs` — 字段路径模型、三方状态读取、脱敏冲突、幂等合并与短事务重校验。
- Modify: `src-tauri/src/codex_profile/route_manager.rs` — token 只读契约、正常关闭与 pending recovery 统一调用。
- Test: `src-tauri/src/codex_profile/home_config.rs` — Home 配置服务单元测试位于同文件 `#[cfg(test)]` 模块。
- Test: `src-tauri/src/codex_profile/route_manager.rs` — 生命周期与补偿测试位于同文件测试模块。

不修改 `schema.rs`、DAO、前端或旧全局 `ProxyService`。

### Task 1: 用失败测试固定字段级恢复语义

**Files:**
- Modify: `src-tauri/src/codex_profile/home_config.rs:497`

- [ ] **Step 1: 添加保留 Codex Desktop 配置和最新 model 的失败测试**

在 `codex_home_config` 测试模块新增测试，先按目标 API 传入 listener token：

```rust
#[test]
fn profile_restore_preserves_current_non_route_fields() -> Result<(), AppError> {
    let home = tempfile::tempdir().expect("创建临时 Home");
    let path = codex_config_path_for_home(home.path());
    let original = r#"model_provider = "custom"
model = "before-model"

[model_providers.custom]
name = "Custom"
base_url = "https://upstream.example/v1"
wire_api = "chat"
experimental_bearer_token = "upstream-token"

[features]
hooks = true
"#;
    fs::write(&path, original).expect("写入接管前配置");
    let service = CodexHomeConfigService::system();
    let plan = service.build_profile_route_plan(
        home.path(),
        15_722,
        None,
        "profile-listener-token",
    )?;
    let backup = service.serialize_backup(&plan)?;
    service.apply_route_plan(&plan)?;

    let routed = fs::read_to_string(&path).expect("读取接管配置");
    let current = routed
        .replace("model = \"before-model\"", "model = \"latest-model\"")
        + r#"
[desktop]
followUpQueueMode = "queue"

[plugins."computer-use@openai-bundled"]
enabled = true

[mcp_servers.node_repl]
command = "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"
"#;
    fs::write(&path, current).expect("模拟 Codex Desktop 写入");

    service.restore_profile_backup(
        home.path(),
        &backup,
        15_722,
        "profile-listener-token",
    )?;

    let restored = fs::read_to_string(&path).expect("读取恢复配置");
    let parsed: toml::Value = toml::from_str(&restored).expect("解析恢复配置");
    assert_eq!(parsed.get("model").and_then(|v| v.as_str()), Some("latest-model"));
    assert_eq!(extract_codex_base_url(&restored).as_deref(), Some("https://upstream.example/v1"));
    assert!(restored.contains("computer-use@openai-bundled"));
    assert!(restored.contains("mcp_servers.node_repl"));
    Ok(())
}
```

- [ ] **Step 2: 添加“previous 缺失字段”和重复恢复测试**

构造 previous 只有 `model` 和 `[features]`，启用路由后追加 Desktop 配置；连续调用两次 `restore_profile_backup`，断言三个严格字段均缺失，Desktop 配置和 model 均保留，第二次不改变字节。

```rust
let once = fs::read(&path).expect("读取第一次恢复结果");
service.restore_profile_backup(home.path(), &backup, 15_722, "profile-listener-token")?;
assert_eq!(fs::read(&path).expect("读取幂等恢复结果"), once);
```

- [ ] **Step 3: 添加三个严格字段逐项冲突的失败测试**

分别把接管配置中的字段改为以下值，并断言恢复失败且字节不变：

```rust
[
    ("base_url", "https://external.example/v1"),
    ("wire_api", "chat"),
    ("experimental_bearer_token", "external-token"),
]
```

token 场景额外断言：

```rust
let message = error.to_string();
assert!(!message.contains("profile-listener-token"));
assert!(!message.contains("external-token"));
```

- [ ] **Step 4: 添加顶层字段与分离路径测试**

覆盖无 `model_provider` 时三个字段均位于顶层；再覆盖保留 provider ID 场景中 `base_url/wire_api` 位于 provider 表、token 位于顶层。恢复后只修改各自目标路径，非目标 provider 表保持不变。

- [ ] **Step 5: 运行新增测试并确认红灯**

Run:

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml codex_home_config::profile_restore --lib
```

Expected: FAIL，因为 `restore_profile_backup` 尚未接受 listener token，且仍使用整文件指纹恢复。

- [ ] **Step 6: 提交测试**

```bash
git add src-tauri/src/codex_profile/home_config.rs
git commit -m "test(codex): 覆盖 Profile 字段级路由恢复"
```

### Task 2: 实现三个字段的纯三方合并

**Files:**
- Modify: `src-tauri/src/codex_profile/constants.rs:14`
- Modify: `src-tauri/src/codex_profile/home_config.rs:60-495`
- Test: `src-tauri/src/codex_profile/home_config.rs:497`

- [ ] **Step 1: 在常量文件定义字段与脱敏标签**

```rust
/// Codex Profile 路由接管的服务地址字段。
pub const CODEX_ROUTE_FIELD_BASE_URL: &str = "base_url";
/// Codex Profile 路由接管的协议字段。
pub const CODEX_ROUTE_FIELD_WIRE_API: &str = "wire_api";
/// Codex Profile 路由接管的本地凭证字段。
pub const CODEX_ROUTE_FIELD_BEARER_TOKEN: &str = "experimental_bearer_token";
/// Codex Profile 本地路由接收的固定 wire API。
pub const CODEX_ROUTE_WIRE_API_RESPONSES: &str = "responses";
/// listener token 缺失或不匹配时使用的脱敏描述。
pub const CODEX_ROUTE_TOKEN_MISMATCH_DETAIL: &str = "本地路由凭证缺失或不匹配";
```

将 `build_codex_route_toml_base` 中新增或本次触及的 `responses` 使用常量，避免继续增加散落字面量；不机械改写无关测试夹具。

- [ ] **Step 2: 定义私有字段路径与状态类型**

在 `CodexRouteBackup` 后新增聚焦的小类型：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum CodexRouteFieldPath {
    TopLevel(&'static str),
    Provider { provider_id: String, field: &'static str },
}

#[derive(Clone, PartialEq)]
enum CodexRouteFieldState {
    Missing,
    Present(toml_edit::Item),
}

#[derive(Clone, PartialEq)]
struct CodexManagedRouteState {
    base_url: CodexRouteFieldState,
    wire_api: CodexRouteFieldState,
    bearer_token: CodexRouteFieldState,
}

#[derive(Clone, PartialEq)]
struct CodexManagedRouteProjection {
    base_url_path: CodexRouteFieldPath,
    wire_api_path: CodexRouteFieldPath,
    bearer_token_path: CodexRouteFieldPath,
    previous: CodexManagedRouteState,
    target: CodexManagedRouteState,
    created_provider_tables: Vec<String>,
    created_model_providers_table: bool,
}
```

状态类型不派生 `Debug`，避免未来调试输出意外泄漏 token；错误格式化必须通过按字段脱敏的专用函数。每个函数只承担一个目标：解析文档、定位路径、读取状态、验证所有权、应用 previous 状态、清理空结构分别实现。

- [ ] **Step 3: 从 previous 纯构造 target 并逐字段定位路径**

实现私有函数：

```rust
fn build_managed_route_projection(
    previous_content: Option<&[u8]>,
    listen_port: u16,
    listener_token: &str,
) -> Result<CodexManagedRouteProjection, AppError>
```

函数必须：

1. 把 previous UTF-8 TOML 解析为 `DocumentMut`，缺失时使用空文档。
2. 调用 `build_codex_profile_route_toml(previous_text, listen_port, None, listener_token)` 得到 target。
3. base/wire 路径按 target 的活动 provider 或顶层定位。
4. token 路径先查 target 活动 provider 表中是否等于 listener token，否则查顶层。
5. 从相同路径分别读取 previous/target 的原始 Item；target 三字段必须存在且为预期字符串，否则返回构造错误。
6. 比较 previous/target 结构，记录本次构造新建的 provider 子表以及 `model_providers` 父表；恢复时只允许清理这些表，previous 原本存在的空表也必须保留。

不得把 token 写入错误文本或 `Debug` 日志。

- [ ] **Step 4: 实现字段级冲突和 previous 应用函数**

实现：

```rust
fn read_managed_route_state(
    document: &toml_edit::DocumentMut,
    projection: &CodexManagedRouteProjection,
) -> CodexManagedRouteState

fn ensure_managed_route_owned(
    current: &CodexManagedRouteState,
    target: &CodexManagedRouteState,
    listen_port: u16,
) -> Result<(), AppError>

fn apply_previous_managed_route_state(
    current: &mut toml_edit::DocumentMut,
    projection: &CodexManagedRouteProjection,
) -> Result<(), AppError>
```

`ensure_managed_route_owned` 按 base URL、wire API、token 顺序返回第一个冲突；字段存在但不是字符串时也按冲突处理，不能与 `Missing` 混淆。token 只使用 `CODEX_ROUTE_TOKEN_MISMATCH_DETAIL`。兼容 `LEGACY_PROXY_MANAGED_TOKEN` 时必须同时要求 base URL 为当前 Profile 端口且 wire API 为 `responses`。

- [ ] **Step 5: 将 Profile backup 恢复改为幂等三方合并**

把签名改为：

```rust
pub fn restore_profile_backup(
    &self,
    home: &Path,
    backup_json: &str,
    listen_port: u16,
    listener_token: &str,
) -> Result<(), AppError>
```

保留 `restore_backup` 和 `restore(plan)` 的既有整文件语义。Profile 专属实现执行：

```rust
let current = self.inspect(home)?;
let projection = build_managed_route_projection(
    backup.previous_content.as_deref(),
    listen_port,
    listener_token,
)?;
if current.content.is_none() {
    return if backup.previous_content.is_none() {
        Ok(())
    } else {
        Err(missing_current_config_error())
    };
}
let mut current_doc = parse_current_document(current.content.as_deref())?;
let current_state = read_managed_route_state(&current_doc, &projection);
if current_state == projection.previous {
    return Ok(());
}
ensure_managed_route_owned(&current_state, &projection.target, listen_port)?;
apply_previous_managed_route_state(&mut current_doc, &projection)?;
```

同步更新 `home_config.rs` 现有直接调用 `restore_profile_backup(home.path(), &backup, 15_722)` 的单测，为其传入生成 target 时使用的同一 listener token，确保旧调用点不遗漏。

- [ ] **Step 6: 增加写入前短事务重校验**

合并结果生成后再次读取 `config.toml`：

```rust
let latest = self.file_ops.read(&current.config_path)?;
ensure_fingerprint(
    &current.fingerprint,
    &fingerprint_content(latest.as_deref()),
)?;
```

随后才调用 `write_atomic`；若合并后文档为空且 previous 文件原本不存在，则调用 `remove_file`。该重校验只保护本轮读取到替换之间的常见竞争，不把长期 Desktop 修改恢复成整文件门槛。

- [ ] **Step 7: 增加缺失文件、异常类型、并发二次读取与原子失败测试**

新增以下测试：

1. previous 与 current 都缺失时幂等成功；previous 存在但 current 缺失时拒绝恢复。
2. current 的任一严格字段为数组、整数等非字符串 Item 时按冲突处理且文件不变；previous 的严格字段 Item 能原样写回。
3. `CodexHomeFileOps` 替身第二次/最终读取返回不同内容，断言 `write_atomic` 未调用。
4. 复用 `FailingRenameFileOps` 断言合并写入失败时当前文件仍存在且 recovery 所需 backup 未变化。

- [ ] **Step 8: 运行 Home 配置测试并确认绿灯**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml codex_home_config --lib
```

Expected: 所有 `codex_home_config` 测试 PASS，三个冲突测试不泄漏 token。

- [ ] **Step 9: 提交字段级合并实现**

```bash
git add src-tauri/src/codex_profile/constants.rs src-tauri/src/codex_profile/home_config.rs
git commit -m "fix(codex): 按路由字段恢复 Profile 配置"
```

### Task 3: 将正常关闭接入只读 token 与统一恢复入口

**Files:**
- Modify: `src-tauri/src/codex_profile/route_manager.rs:70-83`
- Modify: `src-tauri/src/codex_profile/route_manager.rs:591-655`
- Test: `src-tauri/src/codex_profile/route_manager.rs:1787-1802`

- [ ] **Step 1: 为 token store 添加失败测试所需的只读契约**

先在测试中新增 `MissingReadTokenStore`：

```rust
struct MissingReadTokenStore {
    ensured: AtomicUsize,
}

impl CodexProfileTokenStore for MissingReadTokenStore {
    fn read_token(&self, _: &str) -> Result<Option<String>, AppError> {
        Ok(None)
    }

    fn ensure_token(&self, _: &str) -> Result<String, AppError> {
        self.ensured.fetch_add(1, Ordering::SeqCst);
        Ok("unexpected-new-token".to_string())
    }

    fn delete_token(&self, _: &str) -> Result<(), AppError> {
        Ok(())
    }
}
```

新增正常 disable 测试，断言 token 缺失时 route 保持 enabled、`recovery_json` 保留、`ensured == 0`。

- [ ] **Step 2: 运行缺失 token 测试并确认红灯**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml disabling_route_with_missing_token --lib
```

Expected: FAIL，因为 trait 尚无 `read_token`，正常 disable 也尚未读取现有 token。

- [ ] **Step 3: 扩展真实与测试 token store 实现**

修改契约：

```rust
pub trait CodexProfileTokenStore: Send + Sync {
    fn read_token(&self, profile_id: &str) -> Result<Option<String>, AppError>;
    fn ensure_token(&self, profile_id: &str) -> Result<String, AppError>;
    fn delete_token(&self, profile_id: &str) -> Result<(), AppError>;
}
```

真实实现必须调用 `CodexProfileSecretStore::read(profile_id)`。`TrackingTokenStore` 返回 `Some("test-local-token")`，`FailingDeleteTokenStore` 返回相同固定 token；不要用 `ensure_token` 模拟只读。

- [ ] **Step 4: 抽取 manager 私有 Home 恢复入口**

新增单一职责函数：

```rust
fn restore_profile_home_for_disable(
    &self,
    profile: &CodexProfile,
    route: &CodexProfileRoute,
) -> Result<(), AppError>
```

函数在 backup 不存在时返回 `Ok(())`；存在时读取 `read_token`，None 返回脱敏 `AppError::Config`，Some 则调用：

```rust
self.home_config.restore_profile_backup(
    Path::new(&profile.canonical_home_path),
    backup,
    profile.listen_port,
    &listener_token,
)
```

- [ ] **Step 5: 正常 disable 改用统一入口**

在 `persist_operation(... prepared ...)` 之后调用 `restore_profile_home_for_disable`。失败路径继续使用 `persist_operation_error(... disable_home_restore_failed ...)`，保持现有生命周期状态机。

- [ ] **Step 6: 添加正常关闭保留 Desktop 配置的生命周期测试**

准备启用 Profile 后向 Home 追加 `[plugins]`、`[mcp_servers]` 并修改 model；调用 `manager.disable(profile_id)`，断言：

```rust
assert!(!saved_route.enabled);
assert!(saved_route.recovery_json.is_none());
assert!(restored.contains("plugins."));
assert!(restored.contains("mcp_servers."));
assert!(restored.contains("model = \"latest-model\""));
assert!(!restored.contains("http://127.0.0.1:16001/v1"));
```

- [ ] **Step 7: 运行正常关闭与 token 测试**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml disabling_route --lib
```

Expected: 正常关闭、Desktop 保留、缺失 token 三类测试 PASS。

- [ ] **Step 8: 提交 manager 正常关闭集成**

```bash
git add src-tauri/src/codex_profile/route_manager.rs
git commit -m "fix(codex): 关闭 Profile 时读取既有路由凭证"
```

### Task 4: 让 pending disable 与停止失败幂等收敛

**Files:**
- Modify: `src-tauri/src/codex_profile/route_manager.rs:1221-1300`
- Test: `src-tauri/src/codex_profile/route_manager.rs:2360-2495`
- Test: `src-tauri/src/codex_profile/route_manager.rs:2860-2925`

- [ ] **Step 1: 添加当前现场形状的 pending recovery 失败测试**

以既有 `restoring_enabled_profile_home_completes_pending_disable_from_legacy_placeholder` 为邻近参考，新增测试：

1. previous 包含上游 base URL/token。
2. route target 使用本地端口/listener token。
3. current 在 target 后追加 `js_repl`、desktop、plugins、MCP。
4. recovery 为 `disable/disable_home_restore_failed`。
5. 调用 `restore_enabled_profiles`。

断言 route 禁用、recovery 清除、Desktop 段保留、三个字段恢复 previous。

- [ ] **Step 2: 运行 pending 测试并确认红灯**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml pending_disable_preserves_codex_desktop_fields --lib
```

Expected: FAIL，因为 `recover_pending_locked` 尚未调用统一字段级恢复入口。

- [ ] **Step 3: pending disable 改用统一恢复入口**

把 `recover_pending_locked` 中直接调用 `home_config.restore_profile_backup` 的分支替换为 `restore_profile_home_for_disable(&profile, &route)`；错误仍包装为 `recovery_unconverged_error` 并持久化 `disable_home_restore_failed`。

- [ ] **Step 4: 保持旧 `PROXY_MANAGED` 补偿测试通过**

运行既有测试：

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml restoring_enabled_profile_home_completes_pending_disable_from_legacy_placeholder --lib
```

Expected: PASS；同端口旧占位配置仍可收敛。

- [ ] **Step 5: 添加 Home 已恢复、runtime stop 首次失败的重试测试**

使用 `RecoveryRuntime`：第一次 `stop_fails = true`，确认 Home 已恢复且 route phase 为 `disable_stop_failed`；然后设为 false 再调用 disable/recovery，断言字段级恢复识别 previous 状态并完成禁用。

```rust
runtime.stop_fails.store(false, Ordering::SeqCst);
manager.disable("profile-a").await?;
let route = db.get_codex_profile_route("profile-a")?.expect("路由存在");
assert!(!route.enabled);
assert!(route.recovery_json.is_none());
```

- [ ] **Step 6: 添加多 Profile 隔离断言**

创建 A/B 两个临时 Home 与不同端口/token，只关闭 A；断言 B 的配置字节、token read 计数和 runtime 状态均不变。

- [ ] **Step 7: 运行 route manager 全部测试**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib
```

Expected: 所有 route manager 测试 PASS，无 token 明文出现在失败输出。

- [ ] **Step 8: 提交补偿收敛实现**

```bash
git add src-tauri/src/codex_profile/route_manager.rs
git commit -m "fix(codex): 收敛 Profile 未完成关闭恢复"
```

### Task 5: 完整验证与交付检查

**Files:**
- Verify: `src-tauri/src/codex_profile/constants.rs`
- Verify: `src-tauri/src/codex_profile/home_config.rs`
- Verify: `src-tauri/src/codex_profile/route_manager.rs`

- [ ] **Step 1: 格式化 Rust 代码**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml
```

- [ ] **Step 2: 检查格式无漂移**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

Expected: exit 0。

- [ ] **Step 3: 运行 Home 配置服务测试**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml codex_home_config --lib
```

Expected: 0 failed。

- [ ] **Step 4: 运行 route manager 测试**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml codex_profile::route_manager --lib
```

Expected: 0 failed。

- [ ] **Step 5: 运行完整 Codex Profile 测试**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo test --manifest-path src-tauri/Cargo.toml codex_profile --lib
```

Expected: 0 failed。

- [ ] **Step 6: 检查编译与差异质量**

```bash
CC=/usr/bin/cc CXX=/usr/bin/c++ cargo check --manifest-path src-tauri/Cargo.toml
git diff --check
git status --short
```

Expected: `cargo check` 与 `git diff --check` exit 0；状态只包含本计划范围内文件。

- [ ] **Step 7: 审查敏感信息与范围**

```bash
git diff -- src-tauri/src/codex_profile/constants.rs src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/route_manager.rs
rg -n "profile-listener-token|external-token|listener_token" src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/route_manager.rs
```

确认 token 字面量只存在于测试夹具或变量名中，生产错误/日志不拼接 token；确认没有修改 `auth.json`、数据库 schema、前端或旧全局代理。

- [ ] **Step 8: 如验证产生最终小修，提交修正**

仅当 Step 1-7 产生范围内修正时执行：

```bash
git add src-tauri/src/codex_profile/constants.rs src-tauri/src/codex_profile/home_config.rs src-tauri/src/codex_profile/route_manager.rs
git commit -m "test(codex): 完善 Profile 路由恢复验证"
```

- [ ] **Step 9: 交付现场验证说明**

说明新构建安装后，用户可在 CC Switch 中再次关闭 `~/.codex-api` 路由；代码应通过现有 `disable_home_restore_failed` 自动恢复。实施者不得在自动化步骤中直接编辑用户的 `~/.codex-api/config.toml` 或 `~/.cc-switch/cc-switch.db`。
