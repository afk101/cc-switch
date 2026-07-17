# Claude OpenAI Proxy Stream Error Handling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-subagent-driven-development OR superpowers-executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在发送本地流式响应头前校验上游状态，并在流开始后的失败中返回稳定的 OpenAI SSE 错误事件。

**Architecture:** `ClaudeClient` 预先打开并验证上游流，返回拥有幂等关闭能力的流对象；API 层只在预连接成功后创建 `StreamingResponse`。流开始后的异常由独立的 SSE 错误模块分类、脱敏和序列化，不进行自动重试。

**Tech Stack:** Python 3.9+、FastAPI、Starlette、httpx、pytest、OpenAI Chat Completions SSE、Claude Messages SSE

---

### Task 1: 建立可预连接、可关闭的上游流生命周期

**Files:**
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/core/client.py`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_client.py`

- [ ] **Step 1: 编写预连接错误与资源释放失败测试**

在 `tests/test_client.py` 增加使用 `httpx.MockTransport` 的测试，要求 `create_message_stream()` 成为可等待方法，并在返回流对象前抛出上游 HTTP 错误：

```python
def test_create_message_stream_rejects_upstream_error_before_returning_stream():
    async def run_test():
        async def handler(request):
            return httpx.Response(
                408,
                json={"error": {"message": "timeout awaiting response headers"}},
            )

        client = ClaudeClient(
            "secret-key",
            "https://example.com",
            "2023-06-01",
            transport=httpx.MockTransport(handler),
        )

        with pytest.raises(HTTPException) as error:
            await client.create_message_stream({"model": "test", "messages": [], "stream": True}, "req-408")

        assert error.value.status_code == 408
        assert "req-408" not in client.active_requests

    asyncio.run(run_test())
```

再增加正常完成、显式 `aclose()` 和重复 `aclose()` 均只关闭一次资源、清理一次 `active_requests` 的测试。

- [ ] **Step 2: 运行客户端聚焦测试，确认失败**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest tests/test_client.py -q
```

Expected: FAIL，原因是构造器没有 `transport` 参数，且 `create_message_stream()` 仍是异步生成器而非可等待预连接方法。

- [ ] **Step 3: 实现最小上游流对象与预连接方法**

在 `src/core/client.py` 中：

```python
class OpenedClaudeStream:
    """持有已连接的 Claude SSE 响应，并负责幂等释放资源。"""

    async def iter_lines(self) -> AsyncGenerator[str, None]:
        async for line in self.response.aiter_lines():
            if self.cancel_event and self.cancel_event.is_set():
                break
            if line:
                yield f"{line}\n"

    async def aclose(self) -> None:
        if self.closed:
            return
        self.closed = True
        try:
            await self.response.aclose()
        finally:
            try:
                await self.http_client.aclose()
            finally:
                self.on_close()
```

为 `ClaudeClient.__init__()` 增加仅供依赖注入的可选 `transport: Optional[httpx.AsyncBaseTransport] = None`，并拆出 `_create_http_client()`。将 `create_message_stream()` 改为 `async def`：

```python
async def create_message_stream(
    self, claude_request: Dict[str, Any], request_id: Optional[str] = None
) -> OpenedClaudeStream:
    cancel_event = self._register_active_request(request_id)
    http_client = self._create_http_client()
    response = None
    try:
        request = http_client.build_request(
            "POST",
            self.build_messages_url(),
            headers=self.build_headers(request_id),
            json=claude_request,
        )
        response = await http_client.send(request, stream=True)
        if response.status_code >= 400:
            await response.aread()
            self.raise_for_error_response(response)
        return OpenedClaudeStream(
            response,
            http_client,
            cancel_event,
            lambda: self._remove_active_request(request_id),
        )
    except Exception:
        try:
            if response is not None:
                await response.aclose()
        finally:
            try:
                await http_client.aclose()
            finally:
                self._remove_active_request(request_id)
        raise
```

把错误解析拆为只负责校验/抛错的 `raise_for_error_response()`，供 `parse_json_response()` 和流式预连接共同使用。预连接读取响应头的 `httpx.TimeoutException` 映射为 504，其他 `httpx.RequestError` 映射为 502；所有失败路径关闭 response/client 并移除活动请求。

- [ ] **Step 4: 补充配额耗尽分类测试并实现分类**

新增：

```python
def test_classify_claude_error_identifies_exhausted_quota():
    client = ClaudeClient(None, "https://example.com", "2023-06-01")
    assert client.classify_claude_error("claudecodecompat: api key quota exhausted") == (
        "上游 API Key 配额耗尽。请更换可用密钥或等待配额恢复。"
    )
```

在 `classify_claude_error()` 的通用鉴权判断之前匹配 `quota exhausted`，确保不会误报为 API Key 格式无效。

- [ ] **Step 5: 运行客户端测试确认通过**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest tests/test_client.py -q
```

Expected: PASS，预连接失败、错误分类、正常读取和幂等关闭测试全部通过。

- [ ] **Step 6: 提交客户端生命周期改动**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git add src/core/client.py tests/test_client.py
git commit -m "fix(client): validate upstream stream before response"
```

### Task 2: 定义流开始后的 SSE 错误契约

**Files:**
- Create: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/api/stream_errors.py`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/core/constants.py`
- Create: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_stream_errors.py`

- [ ] **Step 1: 编写错误分类与 SSE 格式失败测试**

创建 `tests/test_stream_errors.py`：

```python
import json

import httpx

from src.api.stream_errors import build_stream_error, format_stream_error_sse


def test_timeout_stream_error_has_stable_openai_shape():
    stream_error = build_stream_error(
        httpx.ReadTimeout("timed out with secret-key"),
        "req-timeout",
        "secret-key",
    )
    payload = json.loads(format_stream_error_sse(stream_error).removeprefix("data: ").strip())

    assert payload == {
        "error": {
            "message": "上游流式响应超时，请稍后重试。",
            "type": "upstream_stream_error",
            "code": "upstream_timeout",
            "request_id": "req-timeout",
        }
    }
```

补充网络错误、转换错误，并断言格式化响应与日志字段均不包含 `secret-key`。

- [ ] **Step 2: 运行新测试，确认模块不存在**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest tests/test_stream_errors.py -q
```

Expected: FAIL with `ModuleNotFoundError: No module named 'src.api.stream_errors'`。

- [ ] **Step 3: 实现稳定错误对象、分类和序列化**

在 `src/core/constants.py` 增加：

```python
STREAM_DONE_EVENT = "data: [DONE]\n\n"
STREAM_ERROR_TYPE = "upstream_stream_error"
STREAM_ERROR_TIMEOUT_CODE = "upstream_timeout"
STREAM_ERROR_CONNECTION_CODE = "upstream_connection_error"
STREAM_ERROR_CONVERSION_CODE = "stream_conversion_error"
```

在 `src/api/stream_errors.py` 定义不可变 `StreamError` 数据类，以及职责独立的 `build_stream_error(exception, request_id, api_key)` 和 `format_stream_error_sse()`。`build_stream_error()` 使用现有脱敏逻辑处理诊断字段：超时映射为 `upstream_timeout`，其他 `httpx.RequestError` 映射为 `upstream_connection_error`，转换异常映射为 `stream_conversion_error`；未知异常只返回通用中文提示，不把内部异常正文暴露给调用端。

- [ ] **Step 4: 运行 SSE 错误模块测试确认通过**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest tests/test_stream_errors.py -q
```

Expected: PASS。

- [ ] **Step 5: 提交 SSE 错误契约**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git add src/api/stream_errors.py src/core/constants.py tests/test_stream_errors.py
git commit -m "feat(api): define streaming error events"
```

### Task 3: 在 API 层接入预连接与流中错误结束

**Files:**
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src/api/endpoints.py`
- Modify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests/test_api.py`

- [ ] **Step 1: 编写预连接状态码失败测试**

在 `tests/test_api.py` 中让预连接方法直接抛出上游错误：

```python
def test_streaming_endpoint_returns_upstream_status_before_response_starts(monkeypatch):
    async def fake_create_message_stream(claude_request, request_id=None):
        raise HTTPException(status_code=408, detail="上游请求超时")

    monkeypatch.setattr(endpoints.claude_client, "create_message_stream", fake_create_message_stream)
    response = TestClient(app).post(
        "/v1/chat/completions",
        json={"model": "test-model", "messages": [{"role": "user", "content": "hi"}], "stream": True},
    )

    assert response.status_code == 408
    assert response.json() == {"detail": "上游请求超时"}
```

分别参数化覆盖 401、408、503，证明本地不再先返回 200。

- [ ] **Step 2: 编写流中失败的 SSE 与关闭资源测试**

构造一个 `iter_lines()` 在返回上游 200 后抛出 `httpx.ReadTimeout`、并记录 `aclose()` 调用次数的 fake stream。断言响应状态为 200，SSE 中恰好有一个 `error` 对象和一个 `[DONE]`，不存在 `finish_reason=stop`，且 `aclose()` 恰好执行一次。

- [ ] **Step 3: 运行 API 聚焦测试，确认失败**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest tests/test_api.py -q
```

Expected: FAIL；当前路由没有等待预连接，流中异常仍越过 `StreamingResponse`。

- [ ] **Step 4: API 层等待预连接并安全结束错误流**

把流式分支改为先等待上游：

```python
opened_stream = await claude_client.create_message_stream(claude_request, request_id)
return StreamingResponse(
    _stream_with_disconnect_check(opened_stream, request, http_request, request_id),
    media_type="text/event-stream",
    headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
)
```

在 `_stream_with_disconnect_check()` 中迭代 `opened_stream.iter_lines()`；客户端断开时调用 `cancel_request()`。捕获普通流式异常后调用 `build_stream_error(exception, request_id, claude_client.api_key)`，记录脱敏警告，依次发送 `format_stream_error_sse(error)` 与 `Constants.STREAM_DONE_EVENT`。`finally` 中 `await opened_stream.aclose()`，继续记录 `chat_completion_stream_finished`。不要捕获 `BaseException`，不要把取消信号转换成错误事件。

- [ ] **Step 5: 运行 API 与流错误聚焦测试确认通过**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest tests/test_api.py tests/test_stream_errors.py -q
```

Expected: PASS；预连接错误保留状态码，流中错误输出稳定 SSE 并关闭资源。

- [ ] **Step 6: 提交 API 接入改动**

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git add src/api/endpoints.py tests/test_api.py
git commit -m "fix(api): surface upstream streaming failures"
```

### Task 4: 全量回归与真实日志验收

**Files:**
- Verify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/src`
- Verify: `/Users/qihoo/Documents/A_Own/claude-openai-proxy/tests`
- Verify: `/Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log`

- [ ] **Step 1: 运行完整测试套件**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run pytest -q
```

Expected: 全部 PASS。

- [ ] **Step 2: 编译检查 Python 源码与测试**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
uv run python -m compileall src tests
```

Expected: exit 0，无语法错误。

- [ ] **Step 3: 检查变更范围与格式**

Run:

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git diff --check
git status --short
```

Expected: `git diff --check` 无输出；工作区只包含计划内文件，或在各任务提交后为空。

- [ ] **Step 4: 用可控 fake upstream 验证线上等价链路**

运行 API 测试中的 408/503 与流中超时用例并开启日志捕获，确认包含 `chat_completion_stream_error` 和 request ID，不包含 `Caught handled exception, but response already started`。不直接消耗真实上游配额制造失败。

- [ ] **Step 5: 若实现阶段产生验证修正则提交**

仅当 Task 4 修正了代码或测试时执行：

```bash
cd /Users/qihoo/Documents/A_Own/claude-openai-proxy
git add src tests
git commit -m "test(api): cover streaming failure lifecycle"
```

若没有文件变化，不创建空提交。
