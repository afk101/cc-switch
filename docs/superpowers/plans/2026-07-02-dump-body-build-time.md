# CC_SWITCH_DUMP_BODY 构建时注入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `CC_SWITCH_DUMP_BODY` 从运行时环境变量改为构建时编译期常量

**Architecture:** `build.rs` 在构建阶段读取环境变量并通过 `cargo:rustc-env` 注入默认值，`body_dump.rs` 用 `env!()` 宏在编译期消费该值，替代运行时 `std::env::var()` + `Lazy<bool>` 方案。

**Tech Stack:** Rust, Tauri build script, `env!()` macro

---

### Task 1: build.rs 注入 CC_SWITCH_DUMP_BODY 默认值

**Files:**
- Modify: `src-tauri/build.rs`

- [ ] **Step 1: 在 build.rs 的 main 函数开头添加 rustc-env 注入**

在 `tauri_build::build();` 之后、Windows cfg 块之前添加：

```rust
// 构建时注入 CC_SWITCH_DUMP_BODY：未设置时默认 "0"（关闭）
let dump_body = std::env::var("CC_SWITCH_DUMP_BODY").unwrap_or_else(|_| "0".into());
println!("cargo:rustc-env=CC_SWITCH_DUMP_BODY={dump_body}");
println!("cargo:rerun-if-env-changed=CC_SWITCH_DUMP_BODY");
```

- [ ] **Step 2: 验证 build.rs 编译通过**

Run: `cd src-tauri && cargo check`
Expected: 编译无报错

- [ ] **Step 3: Commit**

```bash
git add src-tauri/build.rs
git commit -m "feat(build): inject CC_SWITCH_DUMP_BODY at compile time via rustc-env"
```

---

### Task 2: body_dump.rs 改用 env!() 编译期常量

**Files:**
- Modify: `src-tauri/src/proxy/body_dump.rs:44-56`

- [ ] **Step 1: 替换 DUMP_ENABLED 定义**

将原来的 `Lazy<bool>` + `std::env::var` 替换为编译期常量：

```rust
// 之前：
// static DUMP_ENABLED: Lazy<bool> = Lazy::new(|| match std::env::var("CC_SWITCH_DUMP_BODY") { ... });

// 之后：构建时由 build.rs 注入，未设置时默认 "0"
const DUMP_ENABLED: bool = {
    let val = env!("CC_SWITCH_DUMP_BODY");
    !val.is_empty() && val != "0" && !val.eq_ignore_ascii_case("false")
};
```

- [ ] **Step 2: 更新 is_enabled() 函数**

```rust
// 之前：
// pub fn is_enabled() -> bool { *DUMP_ENABLED }

// 之后：
#[inline]
pub fn is_enabled() -> bool {
    DUMP_ENABLED
}
```

- [ ] **Step 3: 移除不再需要的 once_cell::sync::Lazy import**

检查 `body_dump.rs` 顶部的 `use once_cell::sync::Lazy;`，如果文件中没有其他地方使用 `Lazy`，则移除该 import。

- [ ] **Step 4: 验证编译通过**

Run: `cd src-tauri && cargo check`
Expected: 编译无报错

- [ ] **Step 5: 运行现有单元测试**

Run: `cd src-tauri && cargo test`
Expected: 所有测试通过

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/proxy/body_dump.rs
git commit -m "refactor(proxy): replace runtime env var with compile-time constant for body dump"
```

---

### Task 3: 验证构建时变量生效

- [ ] **Step 1: 默认构建（关闭 dump）**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 2: 开启 dump 构建**

Run: `cd src-tauri && CC_SWITCH_DUMP_BODY=1 cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 3: Commit（如有文档更新）**

```bash
git add docs/
git commit -m "docs: add dump-body build-time design spec and plan"
```
