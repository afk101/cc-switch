# Codex `/responses` 代理报错：`input[4].call_id` 为空字符串

- 日期：2026-07-01
- 分支：feature/v3.16.4
- 关联现象：CC Switch 本地代理转发 Codex `/responses` 时，上游返回 HTTP 400

## 一、用户提供的第一手信息

- 使用场景：在 **codex 客户端** 中通过 CC Switch 本地代理转发到 provider。
- Provider：`智汇云cc`
- 模型：`cortex-18`
- 上游返回：HTTP 400
- 上游错误原文：
  > Invalid `input[4].call_id`: empty string. Expected a string with minimum length 1, but got an empty string instead.
- 上游 request_id：`chatcmpl-177b861082b6446fafefc4d5830acd2b`
- CC Switch 内部 request id：`cbc91017-39fb-462e-8af9-8918b9950927-SKBrkKOY`

## 二、代码层面的初步定位（自查）

仓库中与该链路直接相关的代码：

- `src-tauri/src/proxy/handlers.rs`：入口
- `src-tauri/src/proxy/providers/transform_responses.rs`：Codex `/responses` 请求转换
- `src-tauri/src/proxy/providers/streaming_responses.rs`：`/responses` 流式响应处理
- `src-tauri/src/proxy/providers/transform_codex_chat.rs`：Codex chat 兼容层转换
- `src-tauri/src/proxy/providers/streaming_codex_chat.rs`：Codex chat 兼容层流式

`call_id` 出现在 Responses API 的 `input` 数组中，用于关联 `function_call` 与 `function_call_output`。`input[4].call_id` 为空字符串通常意味着：

1. 客户端上送 `input` 时该项就是空串；或
2. CC Switch 转换环节生成了空串（例如把原本的 `tool_call_id` / `id` 映射为 `call_id` 时未做非空校验）；或
3. 上游 provider 对 OpenAI Responses 语义要求最小长度 1，但客户端 / 代理侧允许了空串通过。

尚未确认到底是哪一层写入了空串，需要日志佐证。

## 三、待用户提供的信息（继续追问）

1. 复现方式：这个错误是否稳定复现？在什么操作下触发（首轮对话 / 工具调用返回后 / 多轮之后）？
2. 是否使用了工具调用（function calling / MCP）？错误发生在工具调用回合的哪一步？
3. 是否可以打开 CC Switch 的**代理调试日志**并把出错请求前后 20~50 行贴出来？
   - 需要看到：客户端上送的 `input` 序列、CC Switch 转换后发给上游的 body、上游 400 响应体。

## 四、假设候选（待验证，未定论）

- H1：客户端（codex CLI）本身对某个 `function_call_output` 未带 `call_id` / 带空串上送，CC Switch 未校验直接透传。
- H2：CC Switch 在 `transform_responses.rs` / `transform_codex_chat.rs` 中把 `tool_call_id` 转成 `call_id` 时，源字段缺失导致落成空串。
- H3：`智汇云cc` 上游对 `call_id` 做了 minLength=1 校验，但 OpenAI 官方对空串更宽松，导致这条链路只有该 provider 报错。

以上均需日志确认，不做提前修复。

## 五、根本原因

（待确认，尚未定论。）
