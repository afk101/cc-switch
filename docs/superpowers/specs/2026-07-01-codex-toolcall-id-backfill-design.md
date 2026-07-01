# Codex `/responses` tool_call.id 空串双向回填 · 设计文档

- 分支：`feature/v3.16.4`
- 关联 findings：`docs/superpowers/findings/2026-07-01-codex-responses-toolcall-id-findings.md`
- 关联 commit：`1d865b34 feat(proxy): 新增 Codex /responses 链路 body dump 诊断`

## 目标

修复上游 Chat Completions 校验拒收 `messages[N].tool_calls[0].id` / `messages[N+1].tool_call_id` 空串导致的 HTTP 400（现象：`Missing required parameter: 'messages[3].tool_calls[0].id'`）。

## 非目标

- 不修改 `/responses` 原生上游（`should_convert_codex_responses_to_chat` 为 false 的 provider）的行为。
- 不动 `chat_sse_to_response_value_backfills_sparse_tool_call_ids` 已有的非流式聚合兜底逻辑。
- 不新增业务字段、不改 provider 配置。
- 不在本次修复里扩展 `body_dumper` 到 `handle_codex_chat_to_responses_transform` 分支（findings 附带项，另议）。

## 根本原因（引自 findings）

链路：`codex 客户端 → /responses → transform_codex_chat（input→messages）→ 上游 Chat Completions`。

两层原因合并触发 400：

1. **响应侧空 id 下发（B 的 bug 源头）**：`streaming_codex_chat::push_tool_call_delta`（`src-tauri/src/proxy/providers/streaming_codex_chat.rs:429-430`）无条件把 delta 里的 `id` 字符串写入 `state.call_id`。上游 SSE 若显式发送 `id: ""`，会覆盖已有值成空串；tool_call 事件在 `finalize_tools` 里虽有 `format!("call_{key}")` 兜底（727 行），但只在 `state.call_id.is_empty()` 时触发——若上游先给了非空 id、再发了空串 delta，兜底不会触发，结果客户端拿到 `call_id: ""` 的 `output_item.done`。
2. **入站空 id 透传（A 的直接触发点）**：`transform_codex_chat::codex_input_item_to_messages` 分支里，`function_call_output` (`transform_codex_chat.rs:621`)、`custom_tool_call_output` (`:640`)、`responses_function_call_to_chat_tool_call` (`:1136`)、`responses_custom_tool_call_to_chat_tool_call` (`:1157`)、`responses_tool_search_call_to_chat_tool_call` (`:1176`) 都用 `unwrap_or("")` 消化 `call_id` 缺失/为空，直接透传空串进 `messages` 的 `tool_calls[].id` / `tool_call_id`。

## 验收标准

### 功能

- **AC-1（A 立刻救场）**：客户端上送的 `/responses.input` 里带 `call_id: ""` 或 `call_id` 缺失的 `function_call` / `function_call_output` / `custom_tool_call` / `custom_tool_call_output` / `tool_search_call` 条目，经 `transform_codex_chat` 转换后，对应的出站 `messages[N].tool_calls[K].id` 和配对的 `messages[N+1].tool_call_id` 必须为非空且相等。
- **AC-2（配对稳定）**：同一 `input` 中相邻/顺序对应的 `function_call` 与 `function_call_output` 若均空 id，回填后二者的 id 相同（保持配对语义）。
- **AC-3（不破坏已有 id）**：`input` 中已带非空 `call_id` 的 tool call 在转换后 `messages` 里的 id 与入参 `call_id` 完全一致，不被覆盖。
- **AC-4（B 根治源头）**：`streaming_codex_chat` 反向合成 chat SSE → responses SSE 时，若上游 delta 携带 `id` 字段但值为空字符串，不能覆盖已经累积到的非空 `state.call_id`；上游 delta 未携带 `id` 字段时也保持不变；最终 `output_item.done` 与 `output_item.added` 事件中的 `call_id` 一律非空（缺失时按已有 `format!("call_{chat_index}")` 规则回填）。
- **AC-5（上游 400 不再复现）**：使用与本次 bug 相同的坏历史（含 `call_id: ""` 的 `function_call` / `function_call_output` 对）走 `智汇云cc / cortex-18` 链路，代理出站 body 中 `messages[3].tool_calls[0].id` 非空；上游不再返回 `Missing required parameter: 'messages[3].tool_calls[0].id'`。

### 回归

- **AC-6**：`transform_codex_chat` 现有全部单元测试仍绿；`streaming_codex_chat` 现有全部单元测试仍绿。
- **AC-7**：新增 4 组测试（见"测试要求"）全部通过。
- **AC-8**：`cargo clippy --workspace --all-targets --all-features` 无新增警告；`cargo fmt --all -- --check` 通过。

## 修复方案

### A：入站方向兜底（`transform_codex_chat`）

**目标**：在 `input` → `messages` 转换里，对**所有** tool-call 相关条目的 `call_id` 做空值/缺失回填，且 `function_call` 与其配对的 `function_call_output`（以及自定义 tool 的对应输出）复用同一个回填 id。

**位置**：`src-tauri/src/proxy/providers/transform_codex_chat.rs`

- `codex_input_item_to_messages`（覆盖 `Some("function_call_output")` 分支 621 行和 `Some("custom_tool_call_output" | "tool_search_output")` 分支 640 行）。
- `responses_function_call_to_chat_tool_call`（1132 行）。
- `responses_custom_tool_call_to_chat_tool_call`（1156 行）。
- `responses_tool_search_call_to_chat_tool_call`（1175 行）。

**回填 id 生成策略**：

- 首选：使用条目在 `input` 数组中的**索引**生成——`tool_call_{idx}`，`idx` 为该条目在整个 `input` 数组中的 0-based 位置。
- 保证配对：`function_call` 与紧随其后的 `function_call_output` 共享 id，做法是——由调用方（`codex_input_to_messages` 循环层）在遍历 `input` 之前**先做一遍预处理**，扫描出所有 `call_id` 为空或缺失的 tool-call 条目，用「按出现顺序配对」的方式生成同一批 id：
  - 遍历 `input`，遇到 `function_call` / `custom_tool_call` / `tool_search_call` 且 `call_id` 为空 → 生成 `tool_call_{idx}`，压入未消费栈 `pending`；
  - 遇到 `function_call_output` / `custom_tool_call_output` / `tool_search_output` 且 `call_id` 为空 → 从 `pending` 弹出对应 id 使用；若 `pending` 为空则退化为 `tool_call_output_{idx}`（防御式，实际不应出现）。
  - 生成的 id 通过原地修改 `input` 中该条目的 `call_id` 字段来落地，后续所有下游函数（`responses_function_call_to_chat_tool_call` 等）读到的就是已回填的值。这样下游代码不必再改。

**为什么用原地修改而不是给每个下游函数传新参数**：

- 下游 4 个函数是"一进一出、无状态"的语义映射，改签名要动 5+ 个调用点；原地修改让改动集中在 `codex_input_to_messages` 一层。
- `input` 已经是 `serde_json::Value`，即将被消耗构造 `messages`，原地改不影响外部。
- 已有的 `unwrap_or("")` 兜底路径保持不动（不删除既有代码），作为"极端异常仍能返回一个字符串而不 panic"的最后一层安全网。

**SOLID 一致性**：新增一个独立的辅助函数 `backfill_empty_call_ids_in_input(input: &mut Value)`，只做一件事——遍历 + 回填。不做转换、不构造 messages。测试可独立覆盖。

### B：响应方向根治（`streaming_codex_chat::push_tool_call_delta`）

**目标**：不再让上游 SSE 里 `id: ""` 的 delta 把已有的非空 `state.call_id` 冲成空。

**位置**：`src-tauri/src/proxy/providers/streaming_codex_chat.rs:406-410` 附近的 `id_delta` 解析、以及 429-430 行的写入。

**改动**：把

```rust
let id_delta = tool_call
    .get("id")
    .and_then(|v| v.as_str())
    .map(str::to_string);
// ...
if let Some(id) = id_delta {
    state.call_id = id;
}
```

调整为「仅在 delta 携带非空 id 字符串时才覆盖」：

```rust
let id_delta = tool_call
    .get("id")
    .and_then(|v| v.as_str())
    .filter(|v| !v.is_empty())
    .map(str::to_string);
// ...
if let Some(id) = id_delta {
    state.call_id = id;
}
```

`finalize_tools` 727 行的 `format!("call_{key}")` 兜底逻辑保留不改，作为"整个 SSE 生命周期都没见过合法 id"的最后一道兜底。

**为什么不选"忽略所有 id delta"或"每次都回填"**：

- 上游合法行为是"首个 delta 携带 id、后续 delta 只补 arguments"，正常路径不能受影响。
- 只在 delta 明确给空串时才拒收覆盖，语义精确。

### 修复顺序与提交

- 先 A（入站兜底），因为它直接对现存坏会话生效，用户即刻能恢复。
- 再 B（响应根治），防止后续对话再次积累坏历史。
- A、B 分开成两个 commit，便于回退与 code review。

## 测试要求

新增以下测试，且严格遵守 TDD（先失败，后修复，最后通过）。

### T-A1（入站配对回填）

- 位置：`src-tauri/src/proxy/providers/transform_codex_chat.rs` 的 `#[cfg(test)] mod tests`。
- 名称：`codex_input_backfills_empty_call_ids_and_keeps_pairing`。
- 断言：构造 `input` 含 `{function_call, call_id: ""}` + `{function_call_output, call_id: ""}` 一对，经 `codex_request_to_chat`（或最近的模块入口）转换后：
  - `messages[N].tool_calls[0].id == messages[N+1].tool_call_id`
  - 两个 id 都非空
  - 都符合 `^tool_call_\d+$`

### T-A2（不覆盖非空 id）

- 名称：`codex_input_preserves_non_empty_call_ids`。
- 断言：`call_id: "abc123"` 的 `function_call` 转换后 `tool_calls[0].id == "abc123"`。

### T-A3（缺失字段回填）

- 名称：`codex_input_backfills_missing_call_id_field`。
- 断言：完全没有 `call_id` 字段的 `function_call` 也能拿到 `tool_call_{idx}`。

### T-B1（SSE 空串 id delta 不覆盖）

- 位置：`src-tauri/src/proxy/providers/streaming_codex_chat.rs` 的现有测试模块。
- 名称：`streaming_ignores_empty_id_in_tool_call_delta`。
- 输入：两条 SSE，第一条 delta `tool_calls: [{index:0, id:"call_x", function:{name:"f"}}]`，第二条 delta `tool_calls: [{index:0, id:"", function:{arguments:"{}"}}]` + `finish_reason: "tool_calls"`。
- 断言：合成出的 `response.output_item.done` 事件中 `item.call_id == "call_x"`（未被空串覆盖）。

## 观测与回滚

- 观测：用 `CC_SWITCH_DUMP_BODY=1` 复现原来失败的会话，`messages[3].tool_calls[0].id` 应为 `tool_call_3`（或类似 `tool_call_<idx>`），非空。
- 回滚：两个 commit 单独 revert 即可，无 schema/存储改动。

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| 生成的 id 与上游内部标识不同 → 上游是否仍能路由 tool_call | 上游 Chat Completions 只要求 `tool_calls[].id` 与后续 `tool` 消息的 `tool_call_id` 相等即可，与上游内部标识无关，本地生成安全。 |
| `input` 顺序不是严格 `function_call` 紧邻 `function_call_output` | 采用栈式配对（先进后出）→ 若 codex 客户端行为不同，退化为 `tool_call_output_{idx}` 独立 id，output 会与 call 分离，上游仍能通过；补测覆盖不成对场景。 |
| B 分支影响正常流式路径 | 仅在 `id` 字段为空字符串时才不覆盖；已带 id 的场景完全不受影响；补测覆盖。 |

## 引用位置

- 入站接入点：`src-tauri/src/proxy/providers/transform_codex_chat.rs:621`, `:640`, `:1136`, `:1157`, `:1176`。
- 响应根治位置：`src-tauri/src/proxy/providers/streaming_codex_chat.rs:406-410`, `:429-430`。
- 已有非流式兜底（保留不动）：`transform_codex_chat.rs:1433-1438`。
- 已有流式 finalize 兜底（保留不动）：`streaming_codex_chat.rs:727-728`。
- 相关既有测试：`handlers.rs:2254 chat_sse_to_response_value_backfills_sparse_tool_call_ids`。

---

## v2 追加：并行工具调用 FIFO 配对修复

### 背景

commit `17dfc467` 按本 spec v1 实现了入站回填，但使用 `Option<String>` 单槽配对。该方案在串行场景（1 call + 1 output）正常，但在 Codex 客户端发起**并行工具调用**（N 个连续 `function_call` + N 个连续 `function_call_output`，全部空 `call_id`）时配对错乱，上游再次返回 HTTP 400。

详见 `docs/superpowers/findings/2026-07-01-codex-parallel-tool-calls-mismatch-findings.md`。

### 修复方案

将 `backfill_input_item_call_id` 的配对记忆从 `Option<String>` 升级为 `VecDeque<String>`（FIFO 队列）：

- `function_call` 空 call_id → 生成 id，`push_back`
- `function_call_output` 空 call_id → `pop_front`（FIFO 保证顺序配对）
- 非工具项 / 非空 call_id → `.clear()`（打断配对上下文，与 v1 语义一致）

### 改动范围

仅 `src-tauri/src/proxy/providers/transform_codex_chat.rs`：

1. 文件头补 `use std::collections::VecDeque;`
2. 调用方初始化 `let mut pending_call_ids: VecDeque<String> = VecDeque::new();`（替代 `Option<String>`）
3. `backfill_input_item_call_id` 签名 `&mut Option<String>` → `&mut VecDeque<String>`
4. 函数内部：`Some(synthesized)` → `push_back(synthesized)`；`.take()` → `.pop_front()`；`= None` → `.clear()`

### 新增验收标准

- **AC-9（并行配对）**：`input` 中含 2 个连续 `function_call`（空 call_id）+ 2 个连续 `function_call_output`（空 call_id），转换后第 i 个 `tool_calls[i].id` 与第 i 个 `tool` 消息的 `tool_call_id` 相等（FIFO 顺序配对）。

### 新增测试

- **T-A4**：`codex_input_backfills_parallel_tool_calls_with_fifo_pairing` — 2 calls + 2 outputs，断言 FIFO 配对。
