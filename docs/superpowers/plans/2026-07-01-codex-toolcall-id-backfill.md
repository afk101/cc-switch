# Codex `/responses` tool_call.id 空串双向回填 · 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Codex `/responses` 走 chat-completions 转换的 provider（如 智汇云cc / cortex-18）不再因 tool_call.id 为空被上游返 400；同时防止响应侧再向客户端下发空 id。

**Architecture:** 两处修改，两个 commit：A 入站兜底（`transform_codex_chat`）先解决当前坏会话；B 响应根治（`streaming_codex_chat`）防止未来坏历史积累。均遵循 TDD，先测后码。

**Tech Stack:** Rust / Tauri / serde_json / tokio。仅动 `src-tauri/src/proxy/providers/` 下两个文件的目标函数。

---

### Task 1: A-1 — 为入站配对回填新增单元测试（红）

**Files:**
- Modify: `src-tauri/src/proxy/providers/transform_codex_chat.rs`（在末尾 `#[cfg(test)] mod tests` 内追加，不覆盖已有测试）

- [ ] **Step 1: 写入三个失败测试，追加到 `tests` 模块尾部**

在 `mod tests` 尾部追加：

```rust
    /// A-1：配对回填——function_call 与 function_call_output 都是空 call_id 时，
    /// 转换后 tool_calls[0].id 与配对的 tool_call_id 必须同值且非空。
    #[test]
    fn codex_input_backfills_empty_call_ids_and_keeps_pairing() {
        let request = json!({
            "model": "cortex-18",
            "input": [
                { "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
                {
                    "type": "function_call",
                    "call_id": "",
                    "name": "exec_command",
                    "arguments": "{}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "",
                    "output": "ok"
                }
            ]
        });
        let tool_context = crate::proxy::providers::transform_codex_chat::CodexToolContext::default();
        let result = crate::proxy::providers::transform_codex_chat::codex_request_to_chat(
            request,
            &tool_context,
        )
        .expect("transform ok");
        let messages = result["messages"].as_array().expect("messages array");
        // 找到 assistant + tool 这一对
        let (assistant_idx, _) = messages
            .iter()
            .enumerate()
            .find(|(_, m)| m["role"] == "assistant" && m.get("tool_calls").is_some())
            .expect("assistant with tool_calls exists");
        let tool_call_id = messages[assistant_idx]["tool_calls"][0]["id"]
            .as_str()
            .expect("tool_calls[0].id string");
        let tool_msg_id = messages[assistant_idx + 1]["tool_call_id"]
            .as_str()
            .expect("tool_call_id string");
        assert!(!tool_call_id.is_empty(), "tool_calls[0].id 不应为空");
        assert_eq!(tool_call_id, tool_msg_id, "call 与 output 应共享同一个回填 id");
        assert!(
            tool_call_id.starts_with("tool_call_"),
            "回填 id 应符合 tool_call_<idx> 规范，实得 {tool_call_id}"
        );
    }

    /// A-2：非空 call_id 不能被覆盖。
    #[test]
    fn codex_input_preserves_non_empty_call_ids() {
        let request = json!({
            "model": "cortex-18",
            "input": [
                { "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
                {
                    "type": "function_call",
                    "call_id": "abc123",
                    "name": "exec_command",
                    "arguments": "{}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "abc123",
                    "output": "ok"
                }
            ]
        });
        let tool_context = crate::proxy::providers::transform_codex_chat::CodexToolContext::default();
        let result = crate::proxy::providers::transform_codex_chat::codex_request_to_chat(
            request,
            &tool_context,
        )
        .expect("transform ok");
        let messages = result["messages"].as_array().expect("messages array");
        let (assistant_idx, _) = messages
            .iter()
            .enumerate()
            .find(|(_, m)| m["role"] == "assistant" && m.get("tool_calls").is_some())
            .expect("assistant exists");
        assert_eq!(messages[assistant_idx]["tool_calls"][0]["id"], "abc123");
        assert_eq!(messages[assistant_idx + 1]["tool_call_id"], "abc123");
    }

    /// A-3：完全缺失 call_id 字段也要能回填。
    #[test]
    fn codex_input_backfills_missing_call_id_field() {
        let request = json!({
            "model": "cortex-18",
            "input": [
                { "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
                {
                    "type": "function_call",
                    "name": "exec_command",
                    "arguments": "{}"
                }
            ]
        });
        let tool_context = crate::proxy::providers::transform_codex_chat::CodexToolContext::default();
        let result = crate::proxy::providers::transform_codex_chat::codex_request_to_chat(
            request,
            &tool_context,
        )
        .expect("transform ok");
        let messages = result["messages"].as_array().expect("messages array");
        let (assistant_idx, _) = messages
            .iter()
            .enumerate()
            .find(|(_, m)| m["role"] == "assistant" && m.get("tool_calls").is_some())
            .expect("assistant exists");
        let id = messages[assistant_idx]["tool_calls"][0]["id"]
            .as_str()
            .expect("id string");
        assert!(!id.is_empty());
        assert!(id.starts_with("tool_call_"));
    }
```

> 说明：如果 `codex_request_to_chat` 函数名或参数与实际不符（比如实际是 `to_chat_request` / 需 provider 上下文），Step 1 会因编译错误而红。此时先 `grep -n "pub fn.*request.*chat\|pub fn.*codex.*to.*chat" src-tauri/src/proxy/providers/transform_codex_chat.rs` 找到真实入口再改写测试；不要改测试断言本身。测试断言是设计契约。

- [ ] **Step 2: 运行 A-1 测试验证红（必须失败）**

Run:
```bash
cargo test -p cc-switch --lib \
  proxy::providers::transform_codex_chat::tests::codex_input_backfills_empty_call_ids_and_keeps_pairing \
  proxy::providers::transform_codex_chat::tests::codex_input_preserves_non_empty_call_ids \
  proxy::providers::transform_codex_chat::tests::codex_input_backfills_missing_call_id_field
```
Expected：三个测试至少两个 FAIL（`_and_keeps_pairing` 与 `_missing_call_id_field` 必失败，因为出站 id 为空串 / `unwrap_or("")`）；`_preserves_non_empty_call_ids` 应通过。

---

### Task 2: A-2 — 实现入站配对回填（绿）

**Files:**
- Modify: `src-tauri/src/proxy/providers/transform_codex_chat.rs`

- [ ] **Step 1: 新增 `backfill_empty_call_ids_in_input` 辅助函数**

在文件中定位 `codex_request_to_chat`（或等价的把 `request.input` 转成 `messages` 的顶层函数）。在**它进入 `input` 遍历之前**插入一次调用；辅助函数放在该函数之上或同一模块靠近相关辅助的位置。

```rust
/// 遍历 `/responses.input` 数组，把 `function_call` / `custom_tool_call` /
/// `tool_search_call` 及其对应输出条目里空串或缺失的 `call_id` 回填成
/// `tool_call_{idx}`；配对的 output 复用同一个 id，保持 call ↔ output 语义。
///
/// * `input` — `serde_json::Value::Array`；非数组时无副作用。
///
/// 设计选择：原地修改 `input`，让下游 `responses_function_call_to_chat_tool_call`
/// 等函数不必改签名；已有的 `unwrap_or("")` 兜底保留作为最后一道安全网。
fn backfill_empty_call_ids_in_input(input: &mut Value) {
    let Some(items) = input.as_array_mut() else {
        return;
    };
    // 用栈保存"当前已生成、尚未被 output 消费"的 id。
    // 单一会话内 function_call 通常紧跟 function_call_output，栈式配对是安全默认。
    let mut pending_ids: Vec<String> = Vec::new();
    for (idx, item) in items.iter_mut().enumerate() {
        let Some(obj) = item.as_object_mut() else {
            continue;
        };
        let ty = obj
            .get("type")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let is_call = matches!(
            ty.as_deref(),
            Some("function_call") | Some("custom_tool_call") | Some("tool_search_call")
        );
        let is_output = matches!(
            ty.as_deref(),
            Some("function_call_output")
                | Some("custom_tool_call_output")
                | Some("tool_search_output")
        );
        if !is_call && !is_output {
            continue;
        }
        // 是否需要回填：缺失字段或空串。
        let need_backfill = match obj.get("call_id") {
            None => true,
            Some(Value::String(s)) => s.is_empty(),
            Some(_) => false,
        };
        if !need_backfill {
            // 已有非空 id：如果是 call，也压入栈供后续 output 复用；如果是 output，
            // 直接跳过（其 id 已经与之前的 call 对齐）。
            if is_call {
                if let Some(Value::String(s)) = obj.get("call_id") {
                    pending_ids.push(s.clone());
                }
            } else if is_output {
                // 消费一个 pending，如果匹配就弹出；不匹配就不动栈，避免错配。
                if let Some(Value::String(s)) = obj.get("call_id") {
                    if pending_ids.last().map(String::as_str) == Some(s.as_str()) {
                        pending_ids.pop();
                    }
                }
            }
            continue;
        }
        let new_id = if is_call {
            let id = format!("tool_call_{idx}");
            pending_ids.push(id.clone());
            id
        } else {
            // output：优先复用最近一个 pending id，配对丢失时退化为独立 id。
            pending_ids
                .pop()
                .unwrap_or_else(|| format!("tool_call_output_{idx}"))
        };
        obj.insert("call_id".to_string(), Value::String(new_id));
    }
}
```

- [ ] **Step 2: 在 `codex_request_to_chat` 里挂接**

定位 `codex_request_to_chat`（或实际入口）中开始遍历 `input` 之前的位置。如果 `request` 是通过引用/所有权传入的 `Value`，需要在遍历前拿到 `input` 字段的可变引用调用回填：

```rust
// 入站兜底：修 tool_call.id 空串下发上游 400。见 findings 2026-07-01-codex-responses-toolcall-id-findings.md。
if let Some(input) = request.get_mut("input") {
    backfill_empty_call_ids_in_input(input);
}
```

> 若 `request` 是 `&Value` 无法可变，退而把回填结果暂存到本地 `Vec<Value>` 后再传给遍历循环——**不要**把 `unwrap_or("")` 兜底删除或改成 panic，那些是最后一道安全网。

- [ ] **Step 3: 运行 A 组测试验证绿**

Run:
```bash
cargo test -p cc-switch --lib proxy::providers::transform_codex_chat::tests
```
Expected：Task 1 加的三个测试全部 PASS；已有测试全部 PASS。

- [ ] **Step 4: 全量测试与 lint**

Run:
```bash
cargo test -p cc-switch --lib
cargo clippy -p cc-switch --lib -- -D warnings
cargo fmt --all -- --check
```
Expected：全绿。

- [ ] **Step 5: 提交 A**

```bash
git add src-tauri/src/proxy/providers/transform_codex_chat.rs
git commit -m "fix(codex): 入站兜底回填 tool_call.id 空串，修复上游 400

- transform_codex_chat 在 input→messages 前扫描空/缺失 call_id 的
  function_call / function_call_output / custom_tool_call(_output) /
  tool_search_call(_output)，按索引回填 tool_call_{idx}。
- 采用栈式配对，call 与紧随其后的 output 共享同一 id。
- 已有 unwrap_or(\"\") 兜底保留作为最后一道安全网。
- 现象：智汇云cc / cortex-18 走 /responses 时上游返回
  Missing required parameter: 'messages[3].tool_calls[0].id'。

Refs: docs/superpowers/findings/2026-07-01-codex-responses-toolcall-id-findings.md"
```

---

### Task 3: B-1 — SSE 空 id delta 不覆盖测试（红）

**Files:**
- Modify: `src-tauri/src/proxy/providers/streaming_codex_chat.rs`（在现有测试模块尾部追加）

- [ ] **Step 1: 追加失败测试**

在测试模块（`converts_tool_call_chat_sse_to_responses_sse` 附近的 `mod tests`）尾部：

```rust
    /// B-1：上游 SSE 首个 delta 携带非空 id，后续 delta 携带 id:""
    /// 不能覆盖已有的非空 call_id，最终 output_item.done 里必须仍是原 id。
    #[tokio::test]
    async fn streaming_ignores_empty_id_in_tool_call_delta() {
        let chunks = vec![
            "data: {\"id\":\"chatcmpl_x\",\"model\":\"m\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_kept\",\"type\":\"function\",\"function\":{\"name\":\"f\"}}]}}]}\n\n".to_string(),
            "data: {\"id\":\"chatcmpl_x\",\"model\":\"m\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"\",\"function\":{\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n".to_string(),
            "data: [DONE]\n\n".to_string(),
        ];
        // 复用与 converts_tool_call_chat_sse_to_responses_sse 相同的驱动方法，
        // 把 chunks 喂给 create_responses_sse_stream_from_chat_with_context，收集输出。
        let output = drive_stream_and_collect(chunks).await;
        assert!(
            output.contains("\"call_id\":\"call_kept\""),
            "output 应保留原 call_id call_kept，实得：\n{output}"
        );
        assert!(
            !output.contains("\"call_id\":\"\""),
            "output 不应含空串 call_id，实得：\n{output}"
        );
    }
```

> 说明：`drive_stream_and_collect` 是现有测试模块里驱动 SSE 转换的辅助函数（若无同名，就复用 `converts_tool_call_chat_sse_to_responses_sse` 里的相同模式：`futures::stream::iter(chunks.into_iter().map(...))` 喂给 `create_responses_sse_stream_from_chat_with_context`，然后逐帧读取 `bytes` 拼接为 String）。请仿照现有测试的写法。

- [ ] **Step 2: 验证红**

Run:
```bash
cargo test -p cc-switch --lib \
  proxy::providers::streaming_codex_chat::tests::streaming_ignores_empty_id_in_tool_call_delta
```
Expected：FAIL — 输出会包含 `"call_id":""`（因为 `state.call_id = id;` 把 `call_kept` 覆盖成了 `""`）。

---

### Task 4: B-2 — 修复 `push_tool_call_delta`（绿）

**Files:**
- Modify: `src-tauri/src/proxy/providers/streaming_codex_chat.rs:404-410`

- [ ] **Step 1: 只在非空时提取 id_delta**

定位 `fn push_tool_call_delta`（约 404 行）的 `id_delta` 计算：

```rust
        let id_delta = tool_call
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
```

改为：

```rust
        // 上游可能发 `id: ""` 的 delta（如 cortex-18 的旧模型）。忽略空串，
        // 避免覆盖已经累积到的非空 call_id 导致客户端拿到空 id，
        // 进而下一轮 /responses 请求带着空 call_id 触发上游 400。
        let id_delta = tool_call
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_string);
```

429-430 行的写入保持不变。`finalize_tools` 727 行的兜底保留。

- [ ] **Step 2: 验证 B-1 测试绿**

Run:
```bash
cargo test -p cc-switch --lib \
  proxy::providers::streaming_codex_chat::tests::streaming_ignores_empty_id_in_tool_call_delta
```
Expected：PASS。

- [ ] **Step 3: 全量测试与 lint**

Run:
```bash
cargo test -p cc-switch --lib
cargo clippy -p cc-switch --lib -- -D warnings
cargo fmt --all -- --check
```
Expected：全绿。

- [ ] **Step 4: 提交 B**

```bash
git add src-tauri/src/proxy/providers/streaming_codex_chat.rs
git commit -m "fix(codex): 忽略 SSE tool_call delta 里的空串 id 覆盖

上游对 cortex-18 等模型可能在 delta 里发 id:\"\" 覆盖之前已经
累积到的非空 call_id，导致合成出的 response.output_item.done 里
call_id 为空。客户端把这段坏历史存下来后，下一轮 /responses.input
就会带着空 call_id，触发上游 chat/completions 400
'Missing required parameter: messages[3].tool_calls[0].id'。

只在 delta.id 非空字符串时才覆盖 state.call_id；finalize_tools
现有的 format!(\"call_{key}\") 兜底逻辑保留不动。

Refs: docs/superpowers/findings/2026-07-01-codex-responses-toolcall-id-findings.md"
```

---

### Task 5: 端到端手工回归

**Files:** 无

- [ ] **Step 1: 复现原坏会话**

打开 CC Switch，`CC_SWITCH_DUMP_BODY=1` 运行代理，用 codex CLI 恢复出问题的会话（或用 `/tmp/client.txt` 里 dump 出的 body 通过 curl 重放）。

- [ ] **Step 2: 检查最新 dump**

```bash
ls -lt /Users/qihoo/.cc-switch/logs/proxy-bodies/ | head -3
```
选最新一个 `.log` 文件读取 `[CC Switch → Upstream] Request` 段：
- `messages[3].tool_calls[0].id` 应为 `tool_call_<idx>`（非空）。
- 若上下文里存在配对的 `messages[4]`（role: tool），其 `tool_call_id` 应等于 `messages[3].tool_calls[0].id`。

- [ ] **Step 3: 确认上游返回 200**

同一 dump 文件底部（或 CC Switch 主日志）应不再出现 `Missing required parameter: 'messages[3].tool_calls[0].id'`。上游返回 200 或正常 SSE 流。

- [ ] **Step 4: 追加 findings**

把手工回归结果（成功/失败、复现步骤、观察值）追加到 `docs/superpowers/findings/2026-07-01-codex-responses-toolcall-id-findings.md` 的"验证结果"新段。**不新建文件、不覆盖旧内容。**

- [ ] **Step 5: 单独提交 findings 更新**

```bash
git add docs/superpowers/findings/2026-07-01-codex-responses-toolcall-id-findings.md
git commit -m "docs(findings): 追加 tool_call.id 空串修复的端到端回归结果

Refs: docs/superpowers/plans/2026-07-01-codex-toolcall-id-backfill.md"
```

---

## v2 追加：并行工具调用 FIFO 配对修复

> Task 1-5 已在 commit `17dfc467` 中完成（v1 单槽方案）。v2 修复并行工具调用配对错乱。

### Task 6: A-4 — 并行工具调用 FIFO 配对测试（红）

**Files:**
- Modify: `src-tauri/src/proxy/providers/transform_codex_chat.rs`（在 `#[cfg(test)] mod tests` 内追加）

- [ ] **Step 1: 写入失败测试，追加到 `tests` 模块尾部（A-3 测试之后、`}` 之前）**

```rust
    /// A-4：并行工具调用 FIFO 配对——2 个连续 function_call + 2 个连续
    /// function_call_output（全部空 call_id），转换后第 i 个 tool_calls[i].id
    /// 与第 i 个 tool 消息的 tool_call_id 必须按 FIFO 顺序相等。
    #[test]
    fn codex_input_backfills_parallel_tool_calls_with_fifo_pairing() {
        let input = json!({
            "model": "cortex-18",
            "input": [
                { "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
                {
                    "type": "function_call",
                    "call_id": "",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"ls\"}"
                },
                {
                    "type": "function_call",
                    "call_id": "",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "",
                    "output": "file1 file2"
                },
                {
                    "type": "function_call_output",
                    "call_id": "",
                    "output": "/home/user"
                }
            ]
        });
        let result = responses_to_chat_completions(input).expect("transform ok");
        let messages = result["messages"].as_array().expect("messages array");
        // 找到 assistant 消息（含 tool_calls）
        let (assistant_idx, _) = messages
            .iter()
            .enumerate()
            .find(|(_, m)| m["role"] == "assistant" && m.get("tool_calls").is_some())
            .expect("assistant with tool_calls exists");
        let tool_calls = messages[assistant_idx]["tool_calls"]
            .as_array()
            .expect("tool_calls array");
        assert_eq!(tool_calls.len(), 2, "应有 2 个并行 tool_calls");
        // 找到紧随其后的 2 条 tool 消息
        let tool_msg_1 = &messages[assistant_idx + 1];
        let tool_msg_2 = &messages[assistant_idx + 2];
        assert_eq!(tool_msg_1["role"], "tool");
        assert_eq!(tool_msg_2["role"], "tool");
        // FIFO 配对断言：第 i 个 tool_call.id == 第 i 个 tool.tool_call_id
        let call_id_0 = tool_calls[0]["id"].as_str().expect("call 0 id");
        let call_id_1 = tool_calls[1]["id"].as_str().expect("call 1 id");
        let output_id_0 = tool_msg_1["tool_call_id"].as_str().expect("output 0 id");
        let output_id_1 = tool_msg_2["tool_call_id"].as_str().expect("output 1 id");
        assert!(!call_id_0.is_empty());
        assert!(!call_id_1.is_empty());
        assert_ne!(call_id_0, call_id_1, "两个并行 call 应有不同的 id");
        assert_eq!(call_id_0, output_id_0, "FIFO: call#0 应与 output#0 配对");
        assert_eq!(call_id_1, output_id_1, "FIFO: call#1 应与 output#1 配对");
    }
```

- [ ] **Step 2: 运行测试验证红（必须失败）**

Run:
```bash
cargo test -p cc-switch --lib \
  proxy::providers::transform_codex_chat::tests::codex_input_backfills_parallel_tool_calls_with_fifo_pairing
```
Expected：FAIL — 当前 `Option<String>` 单槽方案会导致 `call_id_0 != output_id_0`（错配）或 `output_id_1` 是全新 id（无配对）。

---

### Task 7: A-5 — VecDeque FIFO 队列修复（绿）

**Files:**
- Modify: `src-tauri/src/proxy/providers/transform_codex_chat.rs`

- [ ] **Step 1: 补 `use std::collections::VecDeque`**

在文件头部的 `use` 区域（其他 `std::` 导入附近）添加：

```rust
use std::collections::VecDeque;
```

- [ ] **Step 2: 修改调用方初始化**

定位 `codex_input_to_messages`（或等价入口）中的：

```rust
let mut last_generated_call_id: Option<String> = None;
```

改为：

```rust
// 并行工具调用 FIFO 配对队列：function_call 时 push_back，function_call_output 时 pop_front。
// 见 findings/2026-07-01-codex-parallel-tool-calls-mismatch-findings.md
let mut pending_call_ids: VecDeque<String> = VecDeque::new();
```

同函数内所有传递 `&mut last_generated_call_id` 的调用点改为 `&mut pending_call_ids`（共 2 处，约 563 行和 576 行）。

- [ ] **Step 3: 修改 `backfill_input_item_call_id` 函数签名和内部逻辑**

将整个函数（约 620-671 行）替换为：

```rust
/// 为 `/responses` `input` 数组中 `call_id` 为空或缺失的 `function_call` /
/// `function_call_output` 项补齐兜底 id。
///
/// 配对策略：使用 `pending` FIFO 队列（`VecDeque<String>`）维持并行工具调用的
/// 顺序配对关系：
/// - `function_call` 空 call_id → 生成 id，`push_back` 到队列。
/// - `function_call_output` 空 call_id → `pop_front` 从队列取 id（FIFO 保证
///   第 i 个 call 与第 i 个 output 配对）。
/// - 已有非空 call_id 的工具项 / 非工具项 → `clear` 队列，打断配对上下文。
///
/// 返回值：始终返回一个新的 `Value`（必要时是原始值的 `clone`），保证不会动到
/// 上游持有的原始数组。
fn backfill_input_item_call_id(item: &Value, pending: &mut VecDeque<String>) -> Value {
    let item_type = item.get("type").and_then(|v| v.as_str());
    match item_type {
        Some("function_call") => {
            let mut cloned = item.clone();
            // 仅处理 object；非 object 直接返回，交给下游逻辑按原样处理。
            if let Some(obj) = cloned.as_object_mut() {
                let needs_backfill = obj
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.is_empty())
                    .unwrap_or(true);
                if needs_backfill {
                    let synthesized = generate_backfilled_call_id();
                    obj.insert("call_id".to_string(), Value::String(synthesized.clone()));
                    pending.push_back(synthesized);
                } else {
                    // 已有真实 id：清空队列，避免污染下一对配对。
                    pending.clear();
                }
            }
            cloned
        }
        Some("function_call_output") => {
            let mut cloned = item.clone();
            if let Some(obj) = cloned.as_object_mut() {
                let needs_backfill = obj
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.is_empty())
                    .unwrap_or(true);
                if needs_backfill {
                    // FIFO：从队头取 id 保证与第 i 个 function_call 配对；
                    // 若队列为空（异常序列），退化为独立生成，保底也不出空串。
                    let synthesized = pending
                        .pop_front()
                        .unwrap_or_else(generate_backfilled_call_id);
                    obj.insert("call_id".to_string(), Value::String(synthesized));
                } else {
                    pending.clear();
                }
            }
            cloned
        }
        _ => {
            // 非工具项会打断配对上下文（例如中间穿插了 user/assistant 消息），
            // 清空队列避免跨越消息误复用同一 id。
            pending.clear();
            item.clone()
        }
    }
}
```

- [ ] **Step 4: 更新函数文档注释**

函数上方的 doc comment 已在 Step 3 中一并替换，无需额外操作。

- [ ] **Step 5: 运行 A-4 测试验证绿**

Run:
```bash
cargo test -p cc-switch --lib \
  proxy::providers::transform_codex_chat::tests::codex_input_backfills_parallel_tool_calls_with_fifo_pairing
```
Expected：PASS。

- [ ] **Step 6: 运行全部 A 组测试确认回归**

Run:
```bash
cargo test -p cc-switch --lib proxy::providers::transform_codex_chat::tests
```
Expected：A-1 / A-2 / A-3 / A-4 全部 PASS；已有测试全部 PASS。

- [ ] **Step 7: 全量测试与 lint**

Run:
```bash
cargo test -p cc-switch --lib
cargo clippy -p cc-switch --lib -- -D warnings
cargo fmt --all -- --check
```
Expected：全绿。

- [ ] **Step 8: 提交**

```bash
git add src-tauri/src/proxy/providers/transform_codex_chat.rs
git commit -m "fix(codex): 用 VecDeque FIFO 队列替换单槽配对，修复并行工具调用错配

backfill_input_item_call_id 的 last_generated: Option<String> 只能
记住最近一个生成的 call_id，在并行工具调用场景（N 个连续
function_call + N 个连续 function_call_output，全部空 call_id）
会导致后面的 call 覆盖前面的 id，output 错配到错误的 call。

改用 VecDeque<String> FIFO 队列：call 端 push_back、output 端
pop_front，自然维持第 i 个 call 与第 i 个 output 的顺序配对。

现象：智汇云cc / cortex-18 并行工具调用时上游返回
'No tool output found for function call tool_call_804dfe50...'。

Refs: docs/superpowers/findings/2026-07-01-codex-parallel-tool-calls-mismatch-findings.md"
```
