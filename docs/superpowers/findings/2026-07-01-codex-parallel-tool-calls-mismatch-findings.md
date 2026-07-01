# Codex `/responses` 并行工具调用 call_id 回填配对错乱

- 调查日期：2026-07-01
- 分支：feature/v3.16.4
- 相关 commit：`17dfc467 fix(codex): 回填 responses input 的空 call_id 保证工具调用配对`
- 关联 dump：`/Users/qihoo/.cc-switch/logs/proxy-bodies/20260701-164452-3635440b-9ee1-423d-a437-d3c4294755c6.log`
- 状态：根本原因已确认

## 现象

commit `17dfc467` 修复了空 `call_id` 的问题后，单工具调用场景已恢复正常。但在 Codex 客户端发起**并行工具调用**（同一轮 assistant 消息里包含多个 tool call）时，上游再次返回 HTTP 400：

```
No tool output found for function call tool_call_804dfe50111d442c9aca29fd23a0ade2.
(request_id: chatcmpl-1704fd382f354100af9e5c988bf01973)
(request id: f949bd83-caa3-40d0-b908-eeff07b80eaa-MLFwKPSg)
```

Provider：`智汇云cc`，model：`cortex-18`，upstream_status: HTTP 400。

## 预期行为

并行工具调用场景下（N 个连续 `function_call` + N 个连续 `function_call_output`，所有 `call_id` 为空），回填逻辑应保证第 i 个 `function_call` 与第 i 个 `function_call_output` 共享同一个生成的 `call_id`，使上游收到的 `messages[N].tool_calls[i].id` 与 `messages[N+k].tool_call_id` 正确配对。

## 证据（来自 dump `20260701-164452-3635440b...`）

### 客户端 → CC Switch（input 序列第 637-655 行）

客户端发送了**并行工具调用**——两个连续 `function_call` 后跟两个连续 `function_call_output`，全部 `call_id` 为空：

```
input[?]: { "type": "function_call",         "call_id": "" }   ← 第一个并行调用
input[?]: { "type": "function_call",         "call_id": "" }   ← 第二个并行调用
input[?]: { "type": "function_call_output", "call_id": "" }   ← 第一个并行输出
input[?]: { "type": "function_call_output", "call_id": "" }   ← 第二个并行输出
```

### CC Switch → 上游（messages 序列第 1466-1497 行）

转换后的 Chat Completions messages：

```json
// assistant 消息包含两个 tool_calls
{
  "role": "assistant",
  "tool_calls": [
    { "id": "tool_call_804dfe50111d442c9aca29fd23a0ade2", ... },  // ← call A
    { "id": "tool_call_f4dbd10243664c9ebec33bc19d8707f4", ... }   // ← call B
  ]
},
// tool 响应
{ "role": "tool", "tool_call_id": "tool_call_f4dbd10243664c9ebec33bc19d8707f4" }  // ✓ 匹配 call B
{ "role": "tool", "tool_call_id": "tool_call_471ffc230cf049fcba0212cacae056a6" }  // ✗ 不匹配任何 call！
```

**配对结果**：
| 项 | 期望 id | 实际 id | 结果 |
|---|---|---|---|
| function_call #1 | X | `tool_call_804dfe50...` | — |
| function_call #2 | Y | `tool_call_f4dbd102...` | — |
| function_call_output #1 | X | `tool_call_f4dbd102...` | ✗ 错配到 #2 |
| function_call_output #2 | Y | `tool_call_471ffc23...` | ✗ 全新 id |

## 根本原因

`backfill_input_item_call_id`（`transform_codex_chat.rs:620`）使用 `last_generated_call_id: Option<String>` 作为配对记忆槽——这是一个**单槽**，只能记住最近一次生成的 call_id。

处理序列推演：

```
1. function_call(call_id:"")
   → 生成 tool_call_804dfe50...
   → last_generated = Some("tool_call_804dfe50...")

2. function_call(call_id:"")
   → 生成 tool_call_f4dbd102...
   → last_generated = Some("tool_call_f4dbd102...")  ← 覆盖了 #1 的 id！

3. function_call_output(call_id:"")
   → last_generated.take() → "tool_call_f4dbd102..."  ← 拿到了 #2 的 id（错配）
   → last_generated = None

4. function_call_output(call_id:"")
   → last_generated = None → 独立生成 tool_call_471ffc23...  ← 全新 id（无配对）
```

**核心缺陷**：`Option<String>` 是 LIFO 单槽，无法维持并行工具调用的 FIFO 配对关系。commit `17dfc467` 的注释里提到了"栈式配对"，但实际实现用的是单变量记忆，只覆盖了「一个 function_call 紧跟一个 function_call_output」的串行场景。

## 影响范围

- 场景：Codex 客户端在同一轮 assistant 回复里发起并行工具调用（`multi_tool_use.parallel`），且所有 `call_id` 为空
- 触发条件：连续 ≥2 个 `function_call`（空 call_id）后跟连续 ≥2 个 `function_call_output`（空 call_id）
- 已有测试覆盖缺口：`codex_input_backfills_empty_call_ids_and_keeps_pairing`（A-1）只测了 1 call + 1 output 的串行场景，没有并行用例

## 修复方向

将 `last_generated_call_id: Option<String>` 替换为 `pending_call_ids: VecDeque<String>`（FIFO 队列）：

- `function_call` 空 call_id → 生成 id，`push_back` 到队列
- `function_call_output` 空 call_id → `pop_front` 从队列取 id（FIFO 保证顺序配对）
- 非工具项 / 非空 call_id → `clear` 队列（打断配对上下文）

需新增测试覆盖并行工具调用场景（≥2 calls + ≥2 outputs）。

## 时间线

- 15:10：首次发现空 call_id 问题（dump `20260701-151049`）
- 16:34：commit `17dfc467` 修复空 call_id（单槽配对方案）
- 16:44：修复后首次复现新错误（dump `20260701-164452`），并行调用配对错乱
- 当前：根本原因已确认

---

## 2026-07-01 追加：Brainstorming 结论

用户已确认根本原因，选择 **VecDeque FIFO 队列** 修复方案。

### 方案对比

| 方案 | 描述 | 优点 | 缺点 | 结论 |
|------|------|------|------|------|
| A: VecDeque FIFO | `Option<String>` → `VecDeque<String>`，call 端 push_back、output 端 pop_front | 语义最清晰，标准库原生支持，改动量极小 | 几乎无 | **采纳** |
| B: Vec + 读索引 | `Vec<String>` + `usize` 读指针 | 不引入新类型 | 手动管理索引，容易 off-by-one | 否决 |
| C: 两遍扫描 | 第一遍扫 call 生成 id 列表，第二遍分配给 output | 逻辑最"安全" | 需改调用方，破坏现有纯函数接口 | 否决 |

### 技术决策

| 决策 | 理由 |
|------|------|
| 用 `VecDeque<String>` 替换 `Option<String>` | FIFO 语义天然匹配并行工具调用的顺序配对需求 |
| 只改 `backfill_input_item_call_id` 函数签名 + 内部逻辑 | 改动面最小，调用方只需改初始化一行 |
| 非工具项 / 非空 call_id 时 `.clear()` 队列 | 与原有 `= None` 语义一致，打断配对上下文 |
| 新增并行测试用例（2 calls + 2 outputs） | 已有 A-1 只覆盖串行场景，必须补并行用例 |

### 数据流推演（修复后）

```
1. function_call(call_id:"")  → 生成 id_A → queue: [id_A]
2. function_call(call_id:"")  → 生成 id_B → queue: [id_A, id_B]
3. function_call_output(call_id:"") → pop_front → id_A ✓ 配对 #1
4. function_call_output(call_id:"") → pop_front → id_B ✓ 配对 #2
```
