# Codex Profile Circuit Breaker Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 恢复 Codex Profile 单供应商路由绕过熔断器的旧行为，同时保留多供应商故障转移熔断。

**Architecture:** 使用 Profile 供应商快照的候选数量表达故障转移策略。`ProviderRouter` 在只有一个候选时直接返回主供应商；多个候选继续执行现有熔断过滤，转发器沿用已有的单候选绕过保护。

**Tech Stack:** Rust、Tokio、CC Switch `ProviderRouter`、Codex Profile `RouteRuntime`、Cargo test

---

### Task 1: 固化单供应商与多供应商策略边界

**Files:**
- Modify: `src-tauri/src/codex_profile/route_runtime.rs`
- Test: `src-tauri/src/codex_profile/route_runtime.rs`

- [ ] **Step 1: 编写单供应商失败后仍可选择的回归测试**

在 `route_runtime` 测试模块新增测试，创建只有主供应商的运行时，使用现有 `record_provider_failure` 辅助方法记录足以达到默认阈值的失败后，仍断言主供应商存在：

```rust
#[tokio::test]
async fn single_provider_profile_bypasses_circuit_breaker_after_failure() {
    let db = Arc::new(Database::memory().expect("创建内存数据库"));
    let primary =
        Provider::with_id("primary".to_string(), "主".to_string(), json!({}), None);
    let runtime = RouteRuntime::for_test(
        db,
        CodexRouteProviderSnapshot::new(primary.clone(), Vec::new()),
    );

    runtime.record_provider_failure(&primary.id).await;

    assert_eq!(runtime.select_provider_ids().await, vec![primary.id]);
}
```

- [ ] **Step 2: 编写多供应商仍执行熔断的保护测试**

新增测试，主供应商失败一次后只返回备用供应商：

```rust
#[tokio::test]
async fn multi_provider_profile_still_filters_open_circuit() {
    let db = Arc::new(Database::memory().expect("创建内存数据库"));
    let primary =
        Provider::with_id("primary".to_string(), "主".to_string(), json!({}), None);
    let fallback =
        Provider::with_id("fallback".to_string(), "备".to_string(), json!({}), None);
    let runtime = RouteRuntime::for_test(
        db,
        CodexRouteProviderSnapshot::new(primary.clone(), vec![fallback.clone()]),
    );

    runtime.record_provider_failure(&primary.id).await;

    assert_eq!(runtime.select_provider_ids().await, vec![fallback.id]);
}
```

- [ ] **Step 3: 运行测试并确认单供应商用例失败**

Run:

```bash
cd src-tauri && cargo test single_provider_profile_bypasses_circuit_breaker_after_failure --lib
```

Expected: FAIL，当前 `select_codex_profile_providers` 会过滤唯一的 Open 主供应商，结果为空。

- [ ] **Step 4: 提交回归测试**

```bash
git add src-tauri/src/codex_profile/route_runtime.rs
git commit -m "test(codex): cover single-profile breaker bypass"
```

### Task 2: 恢复单供应商 Profile 绕过逻辑

**Files:**
- Modify: `src-tauri/src/proxy/provider_router.rs`
- Test: `src-tauri/src/codex_profile/route_runtime.rs`

- [ ] **Step 1: 在 Profile 选择边界直接返回唯一候选**

在 `ProviderRouter::select_codex_profile_providers` 开头处理空列表和单元素列表，然后让多元素列表进入现有熔断循环：

```rust
async fn select_codex_profile_providers(
    &self,
    providers: Vec<Provider>,
) -> Result<Vec<Provider>, AppError> {
    match providers.len() {
        0 => return Err(AppError::NoProvidersConfigured),
        1 => return Ok(providers),
        _ => {}
    }

    let total_providers = providers.len();
    // 保留现有多供应商熔断过滤逻辑。
}
```

该分支不得调用 `get_or_create_circuit_breaker`，以保证单供应商选择不会创建或读取熔断状态。

- [ ] **Step 2: 运行两个策略测试**

Run:

```bash
cd src-tauri && cargo test single_provider_profile_bypasses_circuit_breaker_after_failure --lib
cd src-tauri && cargo test multi_provider_profile_still_filters_open_circuit --lib
```

Expected: 两项均 PASS。

- [ ] **Step 3: 提交最小实现**

```bash
git add src-tauri/src/proxy/provider_router.rs src-tauri/src/codex_profile/route_runtime.rs
git commit -m "fix(codex): bypass breaker for single-profile routes"
```

### Task 3: 验证热切换与全部熔断边界

**Files:**
- Modify: `src-tauri/src/codex_profile/route_runtime.rs`
- Test: `src-tauri/src/codex_profile/route_runtime.rs`

- [ ] **Step 1: 编写从多供应商切到单供应商的测试**

创建主、备用两个供应商，使主供应商进入 Open，然后把快照切换为只包含该主供应商，断言新请求仍选择主供应商：

```rust
#[tokio::test]
async fn switching_to_single_provider_ignores_existing_open_state() {
    let db = Arc::new(Database::memory().expect("创建内存数据库"));
    let primary =
        Provider::with_id("primary".to_string(), "主".to_string(), json!({}), None);
    let fallback =
        Provider::with_id("fallback".to_string(), "备".to_string(), json!({}), None);
    let runtime = RouteRuntime::for_test(
        db,
        CodexRouteProviderSnapshot::new(primary.clone(), vec![fallback]),
    );
    runtime.record_provider_failure(&primary.id).await;

    runtime
        .swap_provider_snapshot(CodexRouteProviderSnapshot::new(
            primary.clone(),
            Vec::new(),
        ))
        .await;

    assert_eq!(runtime.select_provider_ids().await, vec![primary.id]);
}
```

- [ ] **Step 2: 编写多供应商全部熔断测试**

将主、备用都记录为失败，直接调用运行时的 `provider_router().select_providers("codex")`，断言错误类型保持为 `AppError::AllProvidersCircuitOpen`：

```rust
let error = runtime
    .server
    .provider_router()
    .select_providers("codex")
    .await
    .expect_err("所有候选 Open 时应返回熔断错误");
assert!(matches!(error, AppError::AllProvidersCircuitOpen));
```

实现时若测试模块无法直接访问 `server`，在 `RouteRuntime` 的 `#[cfg(test)]` 辅助方法中增加返回选择结果的窄接口，不扩大生产 API。

- [ ] **Step 3: 运行 Profile 路由运行时测试**

Run:

```bash
cd src-tauri && cargo test codex_profile::route_runtime::tests --lib
```

Expected: 全部 PASS，包括 Profile 隔离、历史隔离、快照替换和新增熔断策略测试。

- [ ] **Step 4: 提交边界测试**

```bash
git add src-tauri/src/codex_profile/route_runtime.rs
git commit -m "test(codex): cover profile breaker transitions"
```

### Task 4: 完成聚焦与回归验证

**Files:**
- Verify: `src-tauri/src/proxy/provider_router.rs`
- Verify: `src-tauri/src/codex_profile/route_runtime.rs`

- [ ] **Step 1: 检查 Rust 格式**

Run:

```bash
cd src-tauri && cargo fmt --check
```

Expected: PASS。

- [ ] **Step 2: 运行 ProviderRouter 与 Codex Profile 测试**

Run:

```bash
cd src-tauri && cargo test proxy::provider_router::tests --lib
cd src-tauri && cargo test codex_profile --lib
```

Expected: 全部 PASS。

- [ ] **Step 3: 运行 Rust 全量库测试**

Run:

```bash
cd src-tauri && cargo test --lib
```

Expected: 全部现有非忽略测试 PASS；若出现与本变更无关的既有失败，保留完整测试名和错误输出，不修改无关模块。

- [ ] **Step 4: 检查最终差异并提交残余格式变更**

```bash
git diff --check
git status --short
git add src-tauri/src/proxy/provider_router.rs src-tauri/src/codex_profile/route_runtime.rs
git commit -m "chore(codex): finalize profile breaker recovery"
```

如果没有残余改动，不创建空提交。
