# Codex `/responses` 上游报 `tool_calls[0].id` 为空的根因调查

- 调查日期：2026-07-01
- 分支：feature/v3.16.4
- 相关 commit：`1d865b34 feat(proxy): 新增 Codex /responses 链路 body dump 诊断`
- 关联 dump：`/Users/qihoo/.cc-switch/logs/proxy-bodies/*.log`
- 状态：进行中

## 现象（用户第一手信息）

用户从 codex CLI 客户端发起请求，走本地 CC Switch 代理到达上游 `智汇云cc` 的 Codex 兼容端点。CC Switch 日志给出两条不同表述的失败：

1. 早前：`Invalid 'input[4].call_id': empty string. Expected a string with minimum length 1, but got an empty string instead.`（上游走的是原生 `/responses` 校验，报第 4 项 `call_id` 为空串）
2. 现在：`Missing required parameter: 'messages[3].tool_calls[0].id'.`（上游走的是 Chat Completions 校验，报 tool_calls[0] 缺 id）
   - request_id：`cc346bd0-a495-44f9-ada1-3c49737d2ef2-NMBz4OIz`
   - Provider：`智汇云cc`，model：`cortex-18`，HTTP 400

用户已开启 `CC_SWITCH_DUMP_BODY=1` 并成功复现。dump 文件位于 `/Users/qihoo/.cc-switch/logs/proxy-bodies/`：
- `20260701-151049-e15b09df-5334-43b9-90d6-d5c86cbaef11.log`
- `20260701-151019-ba3e587c-db0a-43a0-9d1e-77c077a5da84.log`
- `20260701-151019-399b989b-8d1b-4b2d-9bba-a18af4dc3d5f.log`

## 预期行为

Codex `/responses` 请求应被上游正常处理；如果 provider 需要转成 Chat Completions（`should_convert_codex_responses_to_chat` 匹配），转换后每个 `assistant` 消息里出现的 `tool_calls` 都必须带非空 `id`；对应的 `tool` 消息必须能通过 `tool_call_id` 与之关联。

## 假设初列（待验证）

- H1：客户端上送的 `/responses` body 里，某个 `function_call` 段的 `call_id` 就是空串或缺失；`transform_codex_chat` 直接把它塞进了 `tool_calls[].id`，未做空值兜底。
- H2：客户端 `call_id` 有值，但从 `/responses` → chat 的 mapping 走丢了字段（例如只映射了 `id` 没映射 `call_id`，或反过来）。
- H3：客户端把上一轮的历史消息（`assistant` + `tool_calls`）重发上来，但历史里就没 id（客户端合成或早前被清空过）；此时 CC Switch 也没纠错。
- H4：优化器/整流器（rectifier / optimizer / copilot_optimizer）在转换后二次改写了 body，把 `id` 抹掉。

## 已知代码事实（自查，非用户提供）

- Codex `/responses` handler：`src-tauri/src/proxy/handlers.rs:handle_responses`，转换在 `providers::should_convert_codex_responses_to_chat` 走 `handle_codex_chat_to_responses_transform`。
- 转换实现所在模块：`transform_codex_chat`（含 `build_codex_tool_context_from_request` 等辅助），需要读源码确认 `tool_calls[].id` 的赋值路径。
- body dump 已挂钩：
  - `handlers.rs`：客户端请求。
  - `forwarder.rs`：出站定稿请求（发到上游前的最终 body）。
  - `response_processor.rs`：非流式响应体 + SSE 采样。
- 上游 400 报的是 `messages[3].tool_calls[0]`——即 messages 数组中第 4 条（0-indexed）assistant 消息的第一个 tool_call。

## 下一步操作计划

1. 打开最新的 dump 文件，比对：
   - `[Client → CC Switch] Request`.body 中 `input`（或 `messages`）里 tool-call 相关字段是否有空。
   - `[CC Switch → Upstream] Request`.body 中 `messages[3].tool_calls[0].id` 是否为空。
2. 定位差异层：
   - 客户端就空 → 属于客户端历史合成/回放的问题，CC Switch 需要做兜底。
   - 客户端有值、上游空 → `transform_codex_chat` 有 bug，去精读该模块。
3. 精读 `transform_codex_chat`，验证假设。

## 时间线

- 15:10:19：`399b989b...` dump（第一次失败）。
- 15:10:19：`ba3e587c...` dump（第二次失败）。
- 15:10:49：`e15b09df...` dump（第三次失败，最新，取此文件为主证据）。

## 证据（来自 dump `20260701-151049-e15b09df...`）

### 客户端 → CC Switch（原始入站）

- Method: `POST`；URI: `/v1/responses`（Codex 原生）。
- Body 中 `input` 数组包含 tool 相关项，其中：
  ```json
  { "type": "function_call",       "name": "exec_command", "arguments": "...", "call_id": "" }
  { "type": "function_call_output", "call_id": "", "output": "..." }
  ```
  **两个 `call_id` 都是空串**（`/tmp/client.txt` 第 83、87 行）。

### CC Switch → 上游

- URL: `https://llm.api.zyuncs.com/v1/chat/completions`（由 `providers::should_convert_codex_responses_to_chat` 判定后走 Codex→Chat 转换）。
- Body 中：
  ```json
  messages[3] = { "role": "assistant", "tool_calls": [ { "id": "", "type": "function", "function": {...} } ] }
  messages[4] = { "role": "tool",      "tool_call_id": "", "content": "..." }
  ```
  空串来自入站 body 的 `call_id`。

### 上游响应

- 本次 dump 里**没有** `[Upstream → CC Switch] Response` 段（附带问题，见下）。
- 通过外部日志得知上游返回 HTTP 400：`Missing required parameter: 'messages[3].tool_calls[0].id'`。

## 根本原因

- 客户端在此次会话上下文里，本身就带着 `call_id: ""` 的历史 tool_call 条目上送。此上下文里第一次 tool 调用发生在**上一轮**，那一轮的响应流恐怕没有产出 `call_id`（很可能是 cortex-18 上游把 `function_call` item 的 `call_id` 落空了，或者 CC Switch 的 chat→responses 反向聚合没能回填成非空 id）。codex CLI 把这条空 id 的历史保留，作为下一轮 `input` 回放。
- CC Switch 在 `transform_codex_chat`（源码 `src-tauri/src/proxy/providers/transform_codex_chat.rs` 第 621 / 640 / 1136 / 1157 / 1176 行）里通过 `unwrap_or("")` 消化 `call_id` 缺失，导致空 id 原样透传到 `messages[N].tool_calls[0].id` 和 `messages[N+1].tool_call_id`。上游用 OpenAI Chat Completions 校验直接拒绝。
- 已存在的 `chat_sse_to_response_value_backfills_sparse_tool_call_ids` 只覆盖 **响应侧**（chat SSE → responses value 时回填 `tool_call_{idx}`），**入站请求方向未覆盖**。

## 附带发现：dump 缺失上游响应段

- 三份 dump 全部没有 `===== [Upstream → CC Switch] Response ... =====` 段。
- 推断：`handle_responses` 命中 `should_convert_codex_responses_to_chat` 后跳到 `handle_codex_chat_to_responses_transform`，而后者没走 `process_response`，因此我们上一 commit `1d865b34` 加在 `response_processor.rs` 里的响应侧 dump 逻辑没被触发。修 body dump 补齐这个链路是独立的诊断改进项。

## 影响范围

- 场景：Codex 客户端在多轮上下文中包含 tool_call 历史；且该 tool_call 的 `call_id` 在前一次响应里落空。
- 目前只在走 Codex→Chat 转换的 provider（如 `智汇云cc` 的 `cortex-18`）触发；原生 Codex `/responses` 上游会有 `input[N].call_id` 空串校验，也会失败。
- 客户端在同一会话里会把出问题的历史一直回放 → 每轮都失败，用户表现为「这个会话彻底卡住」。

## 建议后续（不在本 skill 内实施）

1. **入站请求方向补齐 call_id 回填**：在 `transform_codex_chat` 从 `input` 转成 `messages` 之前，扫一遍所有 `function_call` / `function_call_output` 条目，把空 `call_id` 按索引补成 `tool_call_{idx}`（或稳定 hash）；同一 id 的 output 也用同一个值，保持配对。这是最少改动的兜底。
2. **响应侧回填增强**：确认 `handle_codex_chat_to_responses_transform` 生成 responses 结构时，对 chat_sse 里 tool_call.id 缺失/空的场景，都写非空值回给客户端；这样下一轮客户端上送的 `input` 就不会再是空 call_id。
3. **诊断补齐**：`handle_codex_chat_to_responses_transform` 分支下的响应流也接上 `body_dumper`。

- 状态：**根本原因已确认，等待用户确认后进入 brainstorming 讨论修复方案。**

---

## 2026-07-01 追加：Brainstorming 结论

用户已确认根本原因，选择 **A + B 同做** 的修复方向。已产出：

- Spec：`docs/superpowers/specs/2026-07-01-codex-toolcall-id-backfill-design.md`
- Plan：`docs/superpowers/plans/2026-07-01-codex-toolcall-id-backfill.md`

### 精确定位（brainstorming 期间的补充调查）

Brainstorming 阶段进一步阅读了 `streaming_codex_chat.rs` 的反向合成路径，精确定位到 B 分支的 bug 源头：

- **B 的真实 bug 点：`src-tauri/src/proxy/providers/streaming_codex_chat.rs:429-430`**
  ```rust
  if let Some(id) = id_delta {
      state.call_id = id;
  }
  ```
  上游 SSE delta 若显式发送 `id: ""`（字符串空串，非缺失字段），此处会把先前累积的非空 `state.call_id` 覆盖为空。虽然 `finalize_tools:727` 有 `format!("call_{key}")` 兜底，但**只在 `state.call_id.is_empty()` 时触发**——即前后都空才补。若「先非空后空」的顺序出现，最终 `output_item.done` 事件仍带空 call_id 下发给客户端，客户端再回放形成坏历史。
- **B 的非流式路径已经正确**：`transform_codex_chat.rs:1433-1438` 的 `chat_tool_call_to_response_item` 已用 `filter(|v| !v.is_empty()).unwrap_or_else(|| format!("call_{index}"))` 兜底，无需改动。
- **A 的 5 处接入点确认**：`transform_codex_chat.rs` 中 `unwrap_or("")` 的 5 个空 call_id 处理点（621 / 640 / 1136 / 1157 / 1176）全部由**一次 `input` 预处理**覆盖，无需逐点改造。

### 技术决策

| 决策 | 理由 |
|------|------|
| A 在 `codex_input_to_messages` 入口做**原地预处理**回填，而不是改 5 个下游函数签名 | 集中一处、下游语义映射函数保持无状态、`unwrap_or("")` 兜底作为最后一道安全网 |
| A 的 id 格式为 `tool_call_{idx}` | 与已有的 `chat_sse_to_response_value_backfills_sparse_tool_call_ids` 生成的 `tool_call_{idx}` 风格一致 |
| A 采用**栈式配对** | codex 客户端 `function_call` 与 `function_call_output` 通常连续出现；栈能自然维持配对，异常时退化为独立 id |
| B 仅在 `id` 为非空字符串时才覆盖 `state.call_id`，不忽略所有 id delta | 上游合法行为是"首个 delta 携带 id、后续只补 arguments"，正常路径不受影响 |
| 保留所有 `unwrap_or("")` 与 `format!("call_{key}")` 兜底不删 | 遵守用户全局规则「不删既有代码/注释」；作为最后一道安全网 |
| 分两个 commit（A 先 B 后） | A 立刻救活当前坏会话，B 根治源头，各自可独立回退 |

### 遇到的问题

| 问题 | 解决方案 |
|------|---------|
| B 的 bug 起初以为在 `handle_codex_chat_to_responses_transform` 整体逻辑，精读后定位到 `push_tool_call_delta` 的 id 覆盖行 | 通过 `grep call_id` 逐处审阅 `streaming_codex_chat.rs` 状态机 |
| 附带 body dump 不覆盖 chat→responses 反向合成分支 | 记为独立 follow-up，不阻塞本修复 |

### Follow-up（不在本 spec/plan 范围）

- `handle_codex_chat_to_responses_transform` 分支下接入 `body_dumper`，让未来同类问题可以在一份 dump 内看到完整客户端→上游→响应链路。

