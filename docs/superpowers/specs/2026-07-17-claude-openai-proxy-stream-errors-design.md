# Claude OpenAI Proxy 流式错误处理设计

## 背景

Claude OpenAI Proxy 在收到 OpenAI Chat Completions 流式请求后，立即构造 `StreamingResponse`。真正的 Claude Messages 上游连接和状态码检查发生在响应生成器首次迭代时，因此本地 HTTP 200 已经发给调用端。上游随后返回 401、408 或 503 时，生成器抛出 `HTTPException`，Starlette 无法改写已经开始的响应，最终产生 `Caught handled exception, but response already started`，调用端只感知到断流并重新发起请求。

## 目标

- 在发送本地响应头之前完成上游连接和 HTTP 状态码校验。
- 对上游 401、408、503 等预连接错误返回对应 HTTP 状态和可读错误信息。
- 对上游已返回 200 后发生的读取、网络或转换错误，以 OpenAI 兼容 SSE 错误事件结束流。
- 所有流式路径可靠释放上游 response、HTTP client 和活动请求状态。
- 保留现有请求 ID、流式转换摘要和客户端断开取消能力。
- 不增加自动重试，避免工具调用产生重复副作用。

## 非目标

- 不修改模型路由、请求转换、图片能力或 token 上限逻辑。
- 不为 401、408、429、503 增加自动重试、退避或熔断。
- 不修改 cc-switch 的路由配置或 CODEX_HOME 管理逻辑。
- 不掩盖上游错误为普通的成功结束响应。

## 设计

### 1. 上游流生命周期边界

`ClaudeClient.create_message_stream()` 改为可等待的预连接方法：它先创建 `httpx.AsyncClient`，发送 `stream=True` 请求并等待响应头。成功时返回一个拥有上游 response、HTTP client、取消事件和幂等 `aclose()` 的流对象；失败时在返回前释放所有资源并抛出 `HTTPException`。

流对象只负责三件事：逐行读取 SSE、观察取消事件、幂等关闭自身资源。`ClaudeClient` 继续维护 `active_requests`，关闭回调负责移除对应 request ID。HTTP client 创建逻辑单独封装，以便使用 `httpx.MockTransport` 做确定性测试。

### 2. 响应开始前的错误

`create_chat_completion()` 必须先 `await claude_client.create_message_stream(...)`，成功后才构造 `StreamingResponse`。

- 上游返回 HTTP 错误：读取并脱敏错误 body，通过统一错误解析映射为相同 HTTP 状态。
- `quota exhausted`：返回明确的“上游 API Key 配额耗尽”提示，不能误报为密钥格式无效。
- 等待响应头发生 `httpx.TimeoutException`：映射为 504。
- 其他 `httpx.RequestError`：映射为 502。

这些异常发生时本地响应尚未开始，因此 FastAPI 能返回正常 JSON 错误响应，不产生 ASGI 次生异常。

### 3. 响应开始后的错误

`_stream_with_disconnect_check()` 继续包装 Claude-to-OpenAI 转换器。若上游已经返回 200，随后发生读取或转换错误，包装器输出：

```text
data: {"error":{"message":"可读错误信息","type":"upstream_stream_error","code":"upstream_timeout","request_id":"<request-id>"}}

data: [DONE]

```

流中错误不得再输出 `finish_reason=stop` 的成功结束 chunk。错误对象只包含脱敏后的消息、稳定类型/代码和本地 request ID，不包含 API Key、请求正文或完整上游响应。日志记录 `chat_completion_stream_error` 的 request ID、错误代码和异常类型；`finally` 始终记录流结束摘要并调用流对象的幂等关闭方法。

### 4. 常量与职责划分

- `src/core/constants.py`：集中定义 SSE `[DONE]`、错误类型和错误代码常量。
- `src/core/client.py`：负责上游连接、状态检查、取消与资源生命周期。
- `src/api/stream_errors.py`：把异常分类为稳定的流错误对象并格式化 SSE，不依赖 FastAPI 路由状态。
- `src/api/endpoints.py`：编排预连接、`StreamingResponse`、客户端断开和流错误输出。

每个新函数只处理一个目标，错误分类、SSE 序列化和资源关闭分别测试。

## 错误契约

| 场景 | HTTP 状态 | 响应体 |
|---|---:|---|
| 上游预连接返回 401/408/503 | 保留 401/408/503 | FastAPI JSON 错误，包含分类后的可读信息 |
| 等待上游响应头超时 | 504 | FastAPI JSON 错误 |
| 连接上游失败 | 502 | FastAPI JSON 错误 |
| 上游 200 后流读取超时/断开 | 200 | 一条 SSE `error` 事件，随后 `[DONE]` |
| 转换过程中出现异常 | 200 | 脱敏的 SSE `error` 事件，随后 `[DONE]` |
| 客户端主动断开 | 已开始的状态 | 取消上游并关闭资源，不额外发送错误事件 |

## 验收标准

- 模拟上游 401、408、503 时，本地返回对应 HTTP 状态，而不是 200 后断流。
- `quota exhausted` 返回“上游 API Key 配额耗尽”语义。
- 模拟上游 200 后流读取失败时，调用端收到一个可解析的 SSE 错误对象和一个 `[DONE]`。
- 失败流不包含伪造的成功结束 chunk。
- 预连接失败、正常结束、流中失败和客户端取消均关闭上游 response/client，并清理 `active_requests`。
- 日志不再出现 `Caught handled exception, but response already started`。
- 现有非流式请求、正常流式转换及取消行为测试继续通过。
- `uv run pytest` 与 `uv run python -m compileall src tests` 通过。

## 回归风险与控制

- 资源泄漏：用幂等 `aclose()` 和每条退出路径的测试约束。
- 重复 `[DONE]`：成功路径由转换器发送，异常路径仅由流包装器发送；测试精确计数。
- 吞掉取消信号：只捕获普通异常，不捕获 `BaseException`；客户端断开分支继续显式取消。
- 错误信息泄密：继续复用脱敏函数，并以测试验证 API Key 不出现在日志或响应中。
