# Claude OpenAI Proxy 重连排查 Findings

## 用户报告

- 2026-07-17：用户观察到 `/Users/qihoo/Documents/A_Own/sh/logs` 中的 Claude OpenAI Proxy 最近频繁“重连”，询问是否由报错导致。
- 本次仅调查日志与运行状态，不修改服务配置或代码。

## 调查记录

- 日志文件为 `/Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log`，最近写入时间为 2026-07-17 16:06:28。
- 最新请求均为流式 `/v1/chat/completions`。本地先对客户端返回 `200 OK`，再异步建立到 `https://code.jizhi.360.cn/aiproxy/v1/messages` 的上游流。
- 已确认上游失败：多次 `408 Request Timeout`，详情为其内部请求 `https://llm.api.zyuncs.com/v1/chat/completions` 时 `http2: timeout awaiting response headers`；另有 `503 Service Unavailable`，详情为 `upstream error: do request failed`。
- 每次上游 408/503 后，本地会出现 `RuntimeError: Caught handled exception, but response already started.`。堆栈显示异常从 `src/core/client.py:105` 抛出，经 `src/api/endpoints.py:99` 的流式响应传播；这是流式 HTTP 200 已先发出后不能再改写为 HTTP 错误的次生异常。
- 当前监听 7072 的进程为 PID 8015，父进程 PID 为 1，启动时间为 2026-07-17 14:53:31，说明服务当前仍存活。
- 启动脚本仅以 `nohup` 后台运行项目的 `start.sh`；`start.sh` 最终 `exec python -m src.main`，不包含失败后循环重启逻辑。
- 尝试把当前进程段写入 `/tmp` 后再分析被执行环境拒绝（命令中含禁止的删除清理操作），未产生任何文件修改；后续改为纯管道/内存处理，不重复该方式。
- PID 8015 对应的当前进程段内共有：401 18 次、408 9 次、503 3 次，且没有 `Shutting down` 或 `Finished server process` 记录；服务没有因这些异常而重启。
- 所有 401 的具体错误均为 `claudecodecompat: api key quota exhausted`，即该时段上游 API Key 配额耗尽。
- 最近的 408 明确由上游网关转发到 `llm.api.zyuncs.com` 时等待 HTTP/2 响应头超时导致；最近另有 503 上游请求失败。错误后的日志仍出现新的请求与成功的 200 流式响应，进一步说明本地进程持续存活。

## 根本原因

- 用户感知到的“重连”不是本地 Claude OpenAI Proxy 进程反复退出/拉起，而是流式请求在上游 401（额度耗尽）、408（上游超时）或 503（上游不可用）时断开，调用端随后再次发起请求。
- 代理在响应头提前发送 200 后才发现上游错误，导致次生 `RuntimeError`；这会放大日志噪声并使调用端难以获得干净的错误语义，但不是上游失败的原始原因。

## 方案探索

- 当前实现中，`create_chat_completion()` 直接以异步生成器创建 `StreamingResponse`；真正的 `httpx` 上游连接和 HTTP 状态检查位于生成器首次迭代时。因此响应头会先写出 200。
- `ClaudeClient.create_message_stream()` 对 `>=400` 的上游响应读取 body 后抛出 `HTTPException`。这一模式对非流式请求正确，但在流式响应已经开始后无法再转换为 HTTP 状态码。
- 项目已有 `classify_claude_error()`，但它尚未覆盖 `quota exhausted`，且目前没有被流式错误路径调用。
- 最近提交 `f939b9d` 增加了流式转换诊断日志；本次设计应保留这些诊断日志，并避免把上游错误误记为正常流结束。

## 技术决策

| 决策 | 理由 |
|---|---|
| 采用方案 A：流式请求在上游 401/408/503 时输出 OpenAI SSE 错误事件并结束，不做自动重试 | 用户明确选择 A；含工具调用的请求自动重试可能造成重复副作用，且不能解决 API Key 配额耗尽。 |
| 在创建 `StreamingResponse` 前完成上游连接与状态码校验 | 只有此时 FastAPI 仍能正确返回上游 HTTP 状态码，杜绝“响应已开始”的次生异常。 |
| 上游连接成功后发生的传输/转换失败以 SSE 错误事件结束 | HTTP 200 已开始后不能变更状态码；SSE 错误可向客户端明确传达失败并保证流正确结束。 |
| 扩展错误分类以识别 `quota exhausted` | 将 401 的真实额度耗尽原因反馈给调用端，而不是泛化为密钥无效。 |

## 文档规划

- 已校验输出路径：
  - Findings：`docs/superpowers/findings/2026-07-17-claude-openai-proxy-reconnect-findings.md`
  - Spec：`docs/superpowers/specs/2026-07-17-claude-openai-proxy-stream-errors-design.md`
  - Plan：`docs/superpowers/plans/2026-07-17-claude-openai-proxy-stream-errors.md`
- 三条路径均符合 `docs/superpowers/<type>/YYYY-MM-DD-<topic><suffix>.md` 约束。
- 代理仓库当前干净；实现目标文件为 `src/core/client.py`、`src/api/endpoints.py`，测试主要落在 `tests/test_client.py` 与 `tests/test_api.py`。
- cc-switch 工作区还有与本任务无关的用户改动/未跟踪 findings；提交阶段只暂存本任务三份文档，不能带入其他文件。
- 代理仓库及其上级目录没有额外 `AGENTS.md`，实现时遵循现有 Python 模块风格与本任务文档约束。
- 当前 `tests/test_client.py` 仅覆盖非流式错误日志，`tests/test_api.py` 仅覆盖非流式接口；必须新增流式预连接、流中异常、关闭资源和错误分类测试。
- `response_converter.py` 的成功路径会自行发送 `[DONE]`；流中异常路径必须由外层包装器单独发送错误事件和 `[DONE]`，不能再生成成功结束 chunk。
- 自审发现并修正两处生命周期歧义：预连接失败必须显式关闭已创建的 response；`OpenedClaudeStream.aclose()` 即使关闭 response 失败，也必须继续关闭 HTTP client 并清理 `active_requests`。
- 自审将流错误构造器的 API Key 作为显式脱敏参数，保证“响应与日志不泄露密钥”在计划中有可实现的函数签名与测试。
- 第一次最终验证把带换行的暂存文件列表作为一个 `rg` 路径参数，导致占位符和行尾空白扫描未真正执行；该结果作废。后续改为对三个明确路径逐一循环扫描，并让任意匹配或读取失败直接终止。
