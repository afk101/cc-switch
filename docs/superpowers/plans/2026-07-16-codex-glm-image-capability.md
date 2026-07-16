# Codex 360 Text Model Image Guard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 正确标记两个 360 纯文本模型，禁止 Codex 静默图片降级，并让 Python 代理按 Claude 语义合并连续同角色消息。

**Architecture:** CC Switch 在模型能力边界受控归一化 `360-` 路由别名，并让 Codex 图片请求保持原样暴露真实上游错误；独立 Python 代理在逐消息转换后合并相邻同角色回合，保留所有内容块。两个仓库分别测试、分别提交，临时诊断日志在功能提交前清理。

**Tech Stack:** Rust、Serde JSON、Cargo test、Python 3.13、FastAPI、Pydantic、httpx、pytest、uv

---

## 实施前保护

目标仓库：

- `/Users/qihoo/Documents/A_Code/Fork/cc-switch`
- `/Users/qihoo/Documents/A_Own/claude-openai-proxy`

开始前分别运行 `git status --short --branch` 与 `git diff --check`。CC Switch 的另一份 `2026-07-16-codex-stream-response-decode-findings.md` 不属于本计划，禁止暂存。Python 仓库的三处未提交差异是本次调查创建的临时诊断代码，先核对与 findings 记录一致，再使用 `apply_patch` 精确移除；不得使用破坏性 Git 命令。

### Task 1: 用失败测试固定 360 别名与视觉边界

**Files:**
- Modify: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/model_capabilities.rs:217`
- Modify: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/codex_config.rs:2880`
- Test: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/model_capabilities.rs`
- Test: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/codex_config.rs`

- [ ] **Step 1: 增加路由别名注册表失败测试**

在 `model_capabilities.rs` 测试模块增加：

```rust
#[test]
fn confirmed_text_only_registry_accepts_known_360_route_aliases() {
    assert!(is_confirmed_text_only_model("360-glm-5.2"));
    assert!(is_confirmed_text_only_model("360-deepseek-v4-flash"));
    assert!(!is_confirmed_text_only_model("360-glm-5.2v"));
    assert!(!is_confirmed_text_only_model("other-glm-5.2"));
}
```

- [ ] **Step 2: 扩展目录投影测试规格**

在 `catalog_infers_image_input_independently_of_tool_profile` 的 `specs` 中增加无显式模态的 `360-glm-5.2`、`360-deepseek-v4-flash` 和 `360-glm-5.2v`，并在每个 profile 断言：

```rust
assert_eq!(modalities("360-glm-5.2"), json!(["text"]));
assert_eq!(
    modalities("360-deepseek-v4-flash"),
    json!(["text"])
);
assert_eq!(
    modalities("360-glm-5.2v"),
    json!(["text", "image"])
);
```

保留已有显式 visual override 断言。

- [ ] **Step 3: 运行聚焦测试并确认红灯**

```bash
cargo test --manifest-path src-tauri/Cargo.toml model_capabilities::tests::confirmed_text_only_registry_accepts_known_360_route_aliases --lib -- --exact
cargo test --manifest-path src-tauri/Cargo.toml codex_config::tests::catalog_infers_image_input_independently_of_tool_profile --lib -- --exact
```

Expected: 两个测试均因 `360-` 尚未归一化而失败；失败值显示别名仍被输出为图片能力。

### Task 2: 实现受控路由前缀归一化

**Files:**
- Create: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/model_capabilities/constants.rs`
- Modify: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/model_capabilities.rs:1`
- Test: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/model_capabilities.rs`
- Test: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/codex_config.rs`

- [ ] **Step 1: 新建模型能力常量文件**

```rust
//! 模型能力识别使用的领域常量。

/// 已确认由路由层附加、可在精确能力匹配前移除的模型前缀。
pub(super) const KNOWN_ROUTE_MODEL_PREFIXES: &[&str] = &["360-"];
```

- [ ] **Step 2: 添加单一职责前缀归一化函数**

在 `model_capabilities.rs` 声明私有 `constants` 子模块并增加：

```rust
mod constants;

fn strip_known_route_model_prefix(model: &str) -> &str {
    constants::KNOWN_ROUTE_MODEL_PREFIXES
        .iter()
        .find_map(|prefix| model.strip_prefix(prefix))
        .unwrap_or(model)
}
```

`is_confirmed_text_only_model()` 在取得 `/` 尾项后调用该函数，再执行现有 `CONFIRMED_TAILS.contains()`。不要改变显式能力优先级或 `model_ids_match()`。

- [ ] **Step 3: 运行格式化与聚焦测试**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml model_capabilities::tests::confirmed_text_only_registry_accepts_known_360_route_aliases --lib -- --exact
cargo test --manifest-path src-tauri/Cargo.toml codex_config::tests::catalog_infers_image_input_independently_of_tool_profile --lib -- --exact
```

Expected: 格式检查与两个聚焦测试全部通过。

### Task 3: 禁止 Codex 静默图片替换与重试

**Files:**
- Modify: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/proxy/forwarder.rs:184`
- Modify: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/proxy/forwarder.rs:1520`
- Test: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src/proxy/forwarder.rs:4640`

- [ ] **Step 1: 先把 Codex 反应式重试测试改为期望不触发**

把 `reactive_triggers_for_codex_image_url_deserialize_errors` 重命名为 `reactive_skips_codex_image_url_deserialize_errors`，保留原请求体和错误内容，将最终断言改为：

```rust
assert!(!fwd.media_retry_should_trigger("Codex", false, &body, &error));
```

同时增加 `prevention_skips_codex_and_preserves_image_body`：用 `body_with_codex_input_image("360-glm-5.2")` 调用带适配器名称的预防式入口，断言替换数为 0 且内容块类型仍为 `input_image`。

- [ ] **Step 2: 运行该测试并确认红灯**

```bash
cargo test --manifest-path src-tauri/Cargo.toml proxy::forwarder::tests::reactive_skips_codex_image_url_deserialize_errors --lib -- --exact
```

Expected: FAIL，当前实现仍对 Codex 返回 true。

- [ ] **Step 3: 集中实现适配器媒体兜底策略**

增加单一职责策略函数：

```rust
fn adapter_supports_silent_media_fallback(adapter_name: &str) -> bool {
    adapter_name == "Claude"
}
```

`apply_media_prevention()` 增加 `adapter_name` 参数并首先检查该策略；现有 Claude 调用和测试显式传入 `"Claude"`，Codex 调用传入当前 `adapter.name()`。`media_retry_should_trigger()` 使用同一策略函数。保留整流器开关、重复重试保护、图片检测和错误文本检测。

- [ ] **Step 4: 保留 Codex 调用但确保请求体不变**

把 Claude 与 Codex 两个调用点都改为传入 `adapter.name()`。Codex 分支仍经过该入口以保留可观察边界，但策略函数使其返回 0，不修改 `request_body`；Claude 行为保持原样。

- [ ] **Step 5: 运行 forwarder 媒体测试**

```bash
cargo test --manifest-path src-tauri/Cargo.toml proxy::forwarder::tests::reactive_skips_codex_image_url_deserialize_errors --lib -- --exact
cargo test --manifest-path src-tauri/Cargo.toml proxy::forwarder::tests::prevention_skips_codex_and_preserves_image_body --lib -- --exact
cargo test --manifest-path src-tauri/Cargo.toml proxy::forwarder::tests::reactive_triggers_when_all_switches_on --lib -- --exact
cargo test --manifest-path src-tauri/Cargo.toml proxy::forwarder::tests::prevention_replaces_when_all_switches_on_and_model_in_heuristic_list --lib -- --exact
```

Expected: Codex 不重试；Claude 的反应式和预防式兜底继续通过。

- [ ] **Step 6: 提交 CC Switch 功能改动**

```bash
git add src-tauri/src/model_capabilities/constants.rs src-tauri/src/model_capabilities.rs src-tauri/src/codex_config.rs src-tauri/src/proxy/forwarder.rs
git diff --cached --check
git diff --cached --stat
git commit -m "fix(codex): reject images for routed text models"
```

缓存区不得包含 `docs/superpowers/findings/2026-07-16-codex-stream-response-decode-findings.md`。

### Task 4: 清理临时诊断并写连续消息失败测试

**Files:**
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/core/client.py`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/core/constants.py`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_client.py`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_conversion.py`

- [ ] **Step 1: 用 `apply_patch` 精确移除调查诊断代码**

移除以下仅用于本次调查且尚未提交的内容：

- `summarize_claude_request()` 及三个私有摘要辅助函数。
- 流式和非流式发包前的 `log_upstream_request()` 调用及方法。
- `Constants.UPSTREAM_REQUEST_LOG_EVENT`。
- `tests/test_client.py` 中两个摘要日志测试及只为它们添加的 import。

保留原有 `claude_upstream_error` 测试和生产错误日志。

- [ ] **Step 2: 增加连续 user + image 失败测试**

在 `tests/test_conversion.py` 增加：

```python
def test_convert_merges_consecutive_user_messages_without_losing_image():
    request = OpenAIChatCompletionRequest(
        model="360-glm-5.2",
        messages=[
            {"role": "system", "content": "系统规则"},
            {"role": "user", "content": "前置文本"},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "识别图片"},
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": "data:image/png;base64,iVBORw0KGgo="
                        },
                    },
                ],
            },
        ],
    )

    result = convert_openai_to_claude_request(request)

    assert result["messages"] == [
        {
            "role": "user",
            "content": [
                {"type": "text", "text": "前置文本"},
                {"type": "text", "text": "识别图片"},
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "iVBORw0KGgo=",
                    },
                },
            ],
        }
    ]
```

- [ ] **Step 3: 增加连续 tool_result 合并失败测试**

构造一个 assistant 消息包含两个 tool_calls，随后放置两条连续 tool 消息，断言转换结果只有一个 user 回合，content 依次为两个 `tool_result`，且 `tool_use_id` 分别保持原值。

- [ ] **Step 4: 运行聚焦测试并确认红灯**

```bash
uv run pytest tests/test_conversion.py::test_convert_merges_consecutive_user_messages_without_losing_image tests/test_conversion.py::test_convert_merges_consecutive_tool_results -q
```

Expected: 两个测试因当前转换结果仍含连续 user 消息而失败。

### Task 5: 实现 Claude 相邻同角色回合合并

**Files:**
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/conversion/request_converter.py:74`
- Test: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_conversion.py`

- [ ] **Step 1: 增加内容块归一化辅助函数**

```python
def message_content_to_blocks(content: Any) -> List[Dict[str, Any]]:
    """将已转换的 Claude message content 规范为可合并内容块列表。"""
    if isinstance(content, list):
        return list(content)
    if not content:
        return []
    return [{"type": Constants.CONTENT_TEXT, "text": str(content)}]
```

- [ ] **Step 2: 增加相邻同角色合并函数**

```python
def merge_adjacent_messages(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """按 Claude 语义合并相邻同角色消息并保持内容块顺序。"""
    merged: List[Dict[str, Any]] = []
    for message in messages:
        if merged and merged[-1].get("role") == message.get("role"):
            merged[-1]["content"] = [
                *message_content_to_blocks(merged[-1].get("content")),
                *message_content_to_blocks(message.get("content")),
            ]
            continue
        merged.append(dict(message))
    return merged
```

- [ ] **Step 3: 在逐消息转换后调用合并函数**

```python
def convert_openai_messages(messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """转换 OpenAI 消息并合并相邻同角色 Claude 回合。"""
    converted = [convert_openai_message(message) for message in messages]
    return merge_adjacent_messages(converted)
```

- [ ] **Step 4: 增加连续 assistant 内容块测试**

构造相邻 assistant 文本与 tool_call，断言输出只有一个 assistant 回合，内容顺序为 `text`、`tool_use`。该测试应在本步骤实现后直接通过，并与前两个失败测试一起固定通用同角色语义。

- [ ] **Step 5: 运行聚焦和完整测试**

```bash
uv run pytest tests/test_conversion.py -q
uv run pytest -q
uv run python -m compileall src tests
```

Expected: conversion 测试、完整 pytest 和 compileall 全部通过；测试数不少于实施前的 18 个减去两个临时诊断测试再加三个消息归一化测试。

- [ ] **Step 6: 提交 Python 代理功能改动**

```bash
git add src/conversion/request_converter.py tests/test_conversion.py
git diff --cached --check
git diff --cached --stat
git commit -m "fix(conversion): merge adjacent Claude message turns"
```

确认 `src/core/client.py`、`src/core/constants.py`、`tests/test_client.py` 已恢复到调查前状态且工作区无残留诊断差异。

### Task 6: 完整验证与真实链路验收

**Files:**
- Verify: `/Users/qihoo/Documents/A_Code/Fork/cc-switch/src-tauri/src`
- Verify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src`
- Verify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests`

- [ ] **Step 1: 运行 CC Switch 完整 Rust 验证**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

Expected: fmt 退出码 0；lib 测试 0 failed。

- [ ] **Step 2: 运行 Python 代理完整验证**

```bash
uv run pytest -q
uv run python -m compileall src tests
```

Expected: pytest 0 failed；compileall 退出码 0。

- [ ] **Step 3: 检查生成目录行为**

使用聚焦 Rust 测试作为可重复验收证据，确认所有工具 profile 下：

```text
360-glm-5.2              → ["text"]
360-deepseek-v4-flash    → ["text"]
360-glm-5.2v             → ["text", "image"]
```

- [ ] **Step 4: 重启 7072 并发送非流式连续 user 图片请求**

先确认 7072 监听进程，停止旧实例后只启动一个 `/Users/qihoo/Documents/A_Own/claude-openai-proxy/start.sh`。健康检查成功后，向 `http://127.0.0.1:7072/v1/chat/completions` 发送 system、前置 user 文本、第二条 user 图片，使用 `model=360-glm-5.2` 与 `stream=false`。

Expected: 不再返回 200 文本回答；返回 360 上游的图片不支持错误。请求不得包含真实用户图片，使用测试中的 1×1 PNG base64。

- [ ] **Step 5: 检查两个仓库最终状态**

分别运行：

```bash
git status --short --branch
git log -3 --oneline
```

Expected: 两个功能提交存在；CC Switch 只保留任务开始前不属于本计划的未跟踪 stream findings，Python 代理无临时诊断残留；没有推送远端。
