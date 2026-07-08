# Proxy Bodies Daily Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a one-day retention cleanup for `proxy-bodies` logs that runs only when `CC_SWITCH_DUMP_BODY` enables body dump creation.

**Architecture:** Keep the cleanup inside `src-tauri/src/proxy/body_dump.rs`, next to the body dump file creation logic. `BodyDumper::try_new_inner` orchestrates cleanup before creating the new dump file, while small helper functions handle directory scanning and filename date parsing.

**Tech Stack:** Rust, `chrono`, `std::fs`, existing `log` crate, existing Rust unit tests with `tempfile`.

---

## File Structure

- Modify: `src-tauri/src/proxy/body_dump.rs`
  - Add focused constants for dump file extension and date prefix length near the existing module constants.
  - Add `cleanup_old_dump_files(dir: &std::path::Path, today_key: &str)`.
  - Add `dump_file_date_key(path: &std::path::Path) -> Option<&str>` or equivalent owned-return helper if lifetime constraints require it.
  - Call cleanup from `BodyDumper::try_new_inner` after `dump_dir()` and before opening the new file.
  - Add unit tests in the existing `#[cfg(test)] mod tests`.
- Update: `docs/superpowers/findings/2026-07-08-proxy-bodies-size-findings.md`
  - Record final approved方案 A and implementation handoff notes.

## Task 1: Add filename date parsing tests

**Files:**
- Modify: `src-tauri/src/proxy/body_dump.rs:360-428`

- [ ] **Step 1: Write failing tests for dump filename date parsing**

Add tests inside the existing `#[cfg(test)] mod tests`:

```rust
/// dump 文件名应从前 8 位提取日期键。
#[test]
fn dump_file_date_key_extracts_yyyymmdd_prefix() {
    let path = std::path::Path::new("20260707-235959-request.log");
    assert_eq!(dump_file_date_key(path), Some("20260707"));
}

/// 非 log 文件不参与清理。
#[test]
fn dump_file_date_key_ignores_non_log_files() {
    let path = std::path::Path::new("20260707-235959-request.txt");
    assert_eq!(dump_file_date_key(path), None);
}

/// 不符合日期前缀格式的 log 文件不参与清理。
#[test]
fn dump_file_date_key_ignores_malformed_log_names() {
    let path = std::path::Path::new("body-dump.log");
    assert_eq!(dump_file_date_key(path), None);
}
```

- [ ] **Step 2: Run focused Rust test and verify it fails**

Run: `cd src-tauri && cargo test proxy::body_dump::tests::dump_file_date_key_ --lib`

Expected: FAIL because `dump_file_date_key` does not exist yet.

- [ ] **Step 3: Add minimal date parsing helper**

Near existing helper functions, add:

```rust
const DUMP_LOG_EXTENSION: &str = "log";
const DUMP_DATE_KEY_LEN: usize = 8;

/// 从 dump 文件名中提取 `YYYYMMDD` 日期键；无法确认是 dump log 时返回 `None`。
fn dump_file_date_key(path: &std::path::Path) -> Option<&str> {
    if path.extension().and_then(|ext| ext.to_str()) != Some(DUMP_LOG_EXTENSION) {
        return None;
    }

    let file_name = path.file_name()?.to_str()?;
    if file_name.len() <= DUMP_DATE_KEY_LEN || file_name.as_bytes().get(DUMP_DATE_KEY_LEN) != Some(&b'-') {
        return None;
    }

    let date_key = &file_name[..DUMP_DATE_KEY_LEN];
    if date_key.bytes().all(|b| b.is_ascii_digit()) {
        Some(date_key)
    } else {
        None
    }
}
```

- [ ] **Step 4: Run focused Rust test and verify it passes**

Run: `cd src-tauri && cargo test proxy::body_dump::tests::dump_file_date_key_ --lib`

Expected: PASS.

## Task 2: Add cleanup behavior tests

**Files:**
- Modify: `src-tauri/src/proxy/body_dump.rs:360-428`

- [ ] **Step 1: Write failing tests for one-day cleanup**

Add tests inside the existing `#[cfg(test)] mod tests`:

```rust
/// 清理策略删除早于今天的 dump log，保留今天和未来日期的 dump log。
#[test]
fn cleanup_old_dump_files_removes_only_logs_before_today() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old = dir.path().join("20260707-235959-old.log");
    let today = dir.path().join("20260708-000001-today.log");
    let future = dir.path().join("20260709-000001-future.log");
    let malformed = dir.path().join("body-dump.log");
    let note = dir.path().join("20260707-235959-note.txt");

    std::fs::write(&old, "old").expect("write old");
    std::fs::write(&today, "today").expect("write today");
    std::fs::write(&future, "future").expect("write future");
    std::fs::write(&malformed, "malformed").expect("write malformed");
    std::fs::write(&note, "note").expect("write note");

    cleanup_old_dump_files(dir.path(), "20260708");

    assert!(!old.exists());
    assert!(today.exists());
    assert!(future.exists());
    assert!(malformed.exists());
    assert!(note.exists());
}
```

- [ ] **Step 2: Run focused Rust test and verify it fails**

Run: `cd src-tauri && cargo test proxy::body_dump::tests::cleanup_old_dump_files_removes_only_logs_before_today --lib`

Expected: FAIL because `cleanup_old_dump_files` does not exist yet.

- [ ] **Step 3: Add minimal cleanup helper**

Add below `dump_dir()` or near related helpers:

```rust
/// 清理早于今天的 body dump 日志；失败只记录警告，不影响代理主流程。
fn cleanup_old_dump_files(dir: &std::path::Path, today_key: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::warn!("[BodyDump] 读取 dump 目录失败，跳过历史日志清理: {}", dir.display());
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(date_key) = dump_file_date_key(&path) else {
            continue;
        };
        if date_key >= today_key {
            continue;
        }
        if let Err(err) = std::fs::remove_file(&path) {
            log::warn!("[BodyDump] 删除过期 dump 文件失败: {}: {err}", path.display());
        }
    }
}
```

- [ ] **Step 4: Run focused Rust test and verify it passes**

Run: `cd src-tauri && cargo test proxy::body_dump::tests::cleanup_old_dump_files_removes_only_logs_before_today --lib`

Expected: PASS.

## Task 3: Wire cleanup into body dump creation

**Files:**
- Modify: `src-tauri/src/proxy/body_dump.rs:94-102`

- [ ] **Step 1: Add cleanup call before creating the current request file**

Change `BodyDumper::try_new_inner` from:

```rust
let dir = dump_dir()?;
let now = chrono::Local::now();
let file_name = format!("{}-{}.log", now.format("%Y%m%d-%H%M%S"), request_id);
```

to:

```rust
let dir = dump_dir()?;
let now = chrono::Local::now();
let today_key = now.format("%Y%m%d").to_string();
cleanup_old_dump_files(&dir, &today_key);
let file_name = format!("{}-{}.log", now.format("%Y%m%d-%H%M%S"), request_id);
```

- [ ] **Step 2: Run all body dump unit tests**

Run: `cd src-tauri && cargo test proxy::body_dump::tests --lib`

Expected: PASS.

## Task 4: Update findings handoff notes

**Files:**
- Modify: `docs/superpowers/findings/2026-07-08-proxy-bodies-size-findings.md`

- [ ] **Step 1: Append approved design decision**

Append:

```markdown
### 已批准方案

用户选择方案 A：在 `BodyDumper::try_new_inner()` 创建新 dump 文件前，删除早于当天的 `proxy-bodies/*.log`。该路径只会在 `CC_SWITCH_DUMP_BODY` 开启时进入，因此不开启 dump 时没有额外清理行为。
```

- [ ] **Step 2: Inspect git diff for documentation accuracy**

Run: `git diff -- docs/superpowers src-tauri/src/proxy/body_dump.rs`

Expected: Diff matches the approved design and contains no unrelated edits.

## Task 5: Run project verification

**Files:**
- Verify only; no source changes expected in this task.

- [ ] **Step 1: Run typecheck**

Run: `pnpm run typecheck`

Expected: PASS.

- [ ] **Step 2: Run build**

Run: `pnpm run build`

Expected: PASS.

- [ ] **Step 3: Final git diff review**

Run: `git diff --stat && git diff -- src-tauri/src/proxy/body_dump.rs docs/superpowers`

Expected: Only intended body dump cleanup implementation and superpowers docs are changed.
