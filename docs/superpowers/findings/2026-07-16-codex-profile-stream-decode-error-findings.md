# Codex Profile 路由流解码失败调查

## 用户第一手信息

- 调查日期：2026-07-16。
- 现象：Codex 客户端经 `CODEX_HOME` 对应的 Profile 路由请求时失败，客户端报错 `stream disconnected before completion: Stream error: error decoding response body`。
- 用户确认该供应商的上游地址为 `http://127.0.0.1:7072/v1`。
- 用户反馈同一路由在本轮 Profile 关闭恢复改动之前可以使用，现在不可用。
- 截图显示供应商存在多条模型映射，包括 `360-glm-5.2`、`360-deepseek-v4-flash`、`claude-4.8-opus`、`gpt-5.6-*` 与 `gpt-5.5`；尚未确认是否仅特定模型触发。

## 调查边界

- 请求链暂按 `Codex 客户端 -> Profile 独立监听器 -> CC Switch 转换层 -> 127.0.0.1:7072/v1` 追踪。
- 当前阶段只收集证据并确认根因，不修改生产代码或用户的 `CODEX_HOME`、数据库及供应商配置。
- 优先核对最近提交是否触及启用态请求/响应流；若未触及，再检查关闭后重新启用造成的配置或 token/协议变化。

## 待确认

- 错误是每次请求必现，还是仅某个模型、长回复或工具调用触发。
- 本地监听器是否完整收到上游终止事件，以及上游 7072 返回的是 SSE、JSONL、截断 gzip/chunked body，还是连接直接关闭。

## 第一轮证据

- 最近三个实现提交只修改 `codex_profile/constants.rs`、`home_config.rs` 与 `route_manager.rs`；对 `route_runtime.rs`、通用 proxy 请求转换和响应流模块的差异为空。
- 因此“本次提交直接改坏启用态流解析器”暂不成立；仍需检查关闭/重启后 Profile 选中的 provider 快照或配置是否发生变化。
- `~/.cc-switch/logs/proxy-bodies/b341359e-437d-452c-a7ee-1019d99ae939/` 在 13:00-13:03 产生大量新请求日志，说明失败 Profile 的独立监听器确实收到了请求并进入 body dump 链路。
- 主日志 `~/.cc-switch/logs/cc-switch.log` 更新到 13:05，可用于把客户端报错与具体 request id、provider、上游状态对应起来。
## 运行时证据补充

- `~/.cc-switch/logs/cc-switch.log` 在 13:00–13:03 持续记录该 Profile 将 `Auto`、`360-glm-5.2` 请求转发到 `http://127.0.0.1:7072/v1/chat/completions`，说明 Profile 监听器和路由目标选择均已生效。
- 同期最新请求体转储文件大小约为 167–186 KB，目前可见 `Client → CC Switch` 与 `CC Switch → Upstream` 请求段；尚未观察到上游响应段，需要结合转储实现判断这是“响应未返回”还是“转储本来只记录请求”。
- 主日志里暂未找到能直接归因到本次请求的转发错误；大量命中为旧转储清理告警，不能作为当前流断开的根因。
- 对比 11:41 的旧请求与 13:03 的新请求，转储文件都只有两个请求段，没有任何 `Upstream → CC Switch` 响应段；因此不能仅凭“缺少响应段”断定本次回归，但可以确认响应转储阶段没有完成。
- 代码中流式响应正常结束后会由 `response_processor.rs` 调用 `dump_upstream_response_sse` 写入响应段；当前文件止于出站请求，说明链路至少未走到“流式响应正常收尾并落盘”的位置。
- 13:00–13:03 主日志只出现连续出站请求，没有 CC Switch 自身记录的 `流错误` 或首字节/静默期超时；应用层错误更可能发生在建立上游响应或读取 HTTP body 的更底层阶段。
- `create_logged_passthrough_stream` 对上游流的 `Err` 会记录 `流错误`，并且即使异常结束也会在循环后写入 SSE 转储。当前既无该日志又无响应转储，进一步把故障点收窄到进入该透传流之前。
- 端口状态正常：`python -m src.main` 正在监听 `*:7072`，CC Switch 正在监听 Profile 端口 `127.0.0.1:15722`；不是简单的服务未启动或端口未监听。
- 该上游属于 OpenAI/Codex 类后端，CC Switch 使用 reqwest 连接池发送；拿到 2xx 后还会在 `prepare_success_response_for_failover` 中预读流式首块。若此处读取响应体失败，会在进入响应处理器之前返回错误，符合“无透传流错误日志、无响应转储”的现象。
- `7072` 的实际进程是 `/Users/qihoo/Documents/A_Own/claude-openai-proxy` 下的 `python -m src.main`，标准输出写入 `/Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log`；直接访问该端口能收到 Uvicorn 的 HTTP 响应，说明 TCP/HTTP 服务本身可达。
- CC Switch 的首块预读会把底层 body 读取错误包装为 `读取流式响应首包失败`。用户界面的英文 `error decoding response body` 与 reqwest/hyper 读取一个不符合响应头编码/分帧声明的 body 相符，需要用 7072 服务日志确认是其上游断流，还是 CC Switch 改写响应头后造成。
- 已按用户确认的 `/Users/qihoo/Documents/A_Own/sh/logs` 检查；`claude-openai-proxy.log` 正在持续写入，但日志行不使用预期的 `2026-07-16 13:xx` 格式，因此按时间文本检索没有命中，下一步改为读取末尾结构并按请求/异常关键词定位。
- 7072 日志已给出直接根因：每个 `Auto`/`360-glm-5.2` 流式请求先对客户端返回 `200 OK`，随后其上游 `https://code.jizhi.360.cn/aiproxy/v1/messages` 返回 `400 Bad Request`，错误为 `Mismatch type []model.ClaudeContent with value string`，定位在 Claude 请求的 `system` 字段。
- 7072 在响应已经开始后抛出 `HTTPException(400)`，Starlette 再报 `RuntimeError: Caught handled exception, but response already started.`，于是 HTTP 流被非正常截断；这正是 Codex 客户端显示 `stream disconnected ... error decoding response body` 的直接原因。
- 因此该错误文案不是模型返回的正常 SSE 错误，而是 7072 过早发送 200 流响应头、之后无法把上游 400 转换成合法 SSE/HTTP 错误响应造成的二次症状。
- 对比 CC Switch 11:41 的旧转储与 13:03 的新转储，二者出站结构相同：都是 OpenAI Chat `messages`，第一条为 `role=system` 且 `content` 为字符串；没有发现本轮 Profile 恢复改动导致出站 JSON 结构变化。
- 真正把它改成 Claude 顶层 `system` 字符串的是 7072 项目自身的 `convert_openai_to_claude_request`：`merge_system_messages()` 明确定义返回 `Optional[str]`，然后写入 `claude_request["system"]`。而 `code.jizhi.360.cn` 当前报错明确要求 `[]model.ClaudeContent`，即内容块数组。
- 7072 日志的近期上游调用几乎全部为相同的 400，仅夹有一次 200；这表明不是 CC Switch 到 7072 的网络随机断连，而是由具体请求结构触发的稳定校验失败。
- 7072 工作区的 `request_converter.py`（产生错误请求的文件）当前无未提交改动，最近提交历史也没有本次 CC Switch Profile 改动；其未提交文件集中在流转换/断开日志，不涉及 `system` 请求构造。
- 日志中唯一一次上游 200 对应 `message_count=2, tool_count=0` 的简单请求；紧随其后的 Codex Agent 请求为 `message_count=3, tool_count=15`，立即因顶层 `system` 字符串得到 400。说明“端口能用”和“完整 Codex Agent 请求能用”是两个不同条件，故障由完整 Agent 请求携带的 system/tools 路径稳定触发。
- 本轮五个 CC Switch 提交只修改 `codex_profile/constants.rs`、`home_config.rs`、`route_manager.rs` 及测试/文档；对 `forwarder.rs`、`response_processor.rs`、Codex provider 转换代码的合并差异为空。现有证据不支持“本轮代码改坏请求转换器”。
- 当前 `~/.codex-api/config.toml` 的路由字段符合 Profile 路由预期：`model_provider="custom"`、`wire_api="responses"`、`base_url="http://127.0.0.1:15722/v1"`，且 bearer token 存在。Codex Desktop 到 CC Switch 的配置没有被错误改成 7072，也没有把 Codex wire API 改成 chat。
- `wire_api="responses"` 描述的是 Codex 客户端访问 15722 的协议；CC Switch 再依据选中的共享供应商，把 Responses 请求转换为 7072 支持的 `/v1/chat/completions`。日志中的两种路径同时存在是设计行为，不是配置矛盾。
- 当前 Profile `b341359e-437d-452c-a7ee-1019d99ae939` 已启用，绑定共享供应商 `claude-openai-chat`（ID `4847cf65-ad45-4906-bb12-12c3ab228b20`）；2026-07-15 路由恢复前的手工备份中也是同一 Profile、同一供应商且启用，未发生供应商引用串换。
- 尝试用 jq 枚举供应商 JSON 路径时因 shell 转义写法错误而失败；不会重复该命令，改用 SQLite 内置 `json_tree` 直接枚举并按字段名脱敏。旧备份的 JSON 查询路径也未命中，说明假设的 `$.config.*` 层级不正确，需先读取实际结构再查询。
- SQLite `json_tree` 已确认共享供应商 `claude-openai-chat` 的配置正文为 `wire_api="responses"`、`base_url="http://127.0.0.1:7072/v1"`；供应商全局配置与 Profile 中引用的对象一致，模型目录本身不参与 system 消息结构转换。
- 7072 日志在本次服务启动之前没有同格式的 httpx 上游状态记录，无法仅靠这一个日志文件证明远端何时开始拒绝字符串 system；能确认的是当前服务启动后，完整 Agent 请求稳定 400，只有不携带 tools/system 路径的简单请求成功。

## Brainstorming 阶段补充

### 现有实现边界

- 修复落点实际位于独立仓库 `/Users/qihoo/Documents/A_Own/claude-openai-proxy`，CC Switch 只负责把 Codex Responses 转成 OpenAI Chat 并路由到 7072。
- 7072 的 `create_chat_completion` 在构造 `StreamingResponse` 时仅创建异步生成器，上游 HTTP 请求要等响应体开始迭代后才真正执行，因此 FastAPI 已发送 200 响应头后才可能发现上游 4xx。
- `ClaudeClient.create_message_stream` 直接在异步生成器内部将上游 4xx 抛为 `HTTPException`；这个异常越过已经开始的 `StreamingResponse` 后形成 `Caught handled exception, but response already started`。
- 当前测试明确断言 Claude 顶层 `system` 是字符串，因此需要更新既有契约测试，而不是只新增实现代码；当前没有覆盖“流式上游在首包前返回 4xx 时，入口不得先返回 200”的测试。
- `claude-openai-proxy` 工作区已有用户未提交改动：`src/api/endpoints.py`、`src/conversion/response_converter.py`、`tests/test_conversion.py`。后续计划必须保留并基于这些改动工作，不能覆盖或回滚。

### 待决策范围

- 只修复 system 内容块兼容即可恢复当前请求；同时修复流式预检可避免未来任何上游 4xx 再被伪装成客户端 body decode 错误。需要明确本次是否两层一起完成。

### 用户确认的范围

- 本次只修复第 1 层：将 Claude 顶层 `system` 改为上游要求的内容块数组。
- 明确不修改流式响应预检与 4xx 错误传播；`error decoding response body` 的二次错误机制保留为已知问题，不纳入本次验收标准。

### 候选方案

| 方案 | 做法 | 权衡 |
|------|------|------|
| A. 始终输出 Claude 内容块数组（推荐） | 合并所有 OpenAI system 消息后输出 `[{"type":"text","text":"..."}]` | 改动最小；满足当前智汇上游的严格类型要求；Claude Messages 兼容上游通常也支持内容块数组 |
| B. 首次字符串、类型错误后数组重试 | 保留旧请求，匹配特定 400 后重发 | 兼容旧行为，但会重复发送请求、增加延迟，并依赖上游错误文本，不适合流式路径 |
| C. 增加 system 格式配置项 | 每个部署选择 string 或 blocks | 兼容性最强，但增加配置、文档和测试负担；当前只有一个明确上游要求，超出本次 YAGNI 范围 |

### 推荐决策

- 推荐方案 A：在 `request_converter.py` 中让 system 合并函数返回单个 `text` 内容块数组，并同步更新 API/转换契约测试。
- 理由：当前失败是确定的类型不匹配，不需要运行时探测或新增配置；固定使用标准化内容块能以最小变更恢复完整 Codex Agent 请求。

### 已批准的设计边界

- 用户批准方案 A 及转换边界：`merge_system_messages()` 合并文本后返回单个 `text` 内容块数组；无有效文本时返回 `None`；其余请求字段与 CC Switch 均不修改。
- 现有 `tests/test_conversion.py` 的工具调用综合用例与 `tests/test_api.py` 的入口契约用例都明确断言 system 字符串，必须同步改成内容块数组，防止只改单元测试而遗漏 API 传递契约。
- 外部仓库路径上未发现额外 `AGENTS.md` 约束；执行阶段仍须遵守当前 cc-switch 会话的工作区保护规则，并保留外部仓库现有未提交改动。
- 用户批准测试与验收设计：覆盖转换契约、API 传递契约、多 system 合并、空 system 省略和其他字段不回归；最终需通过外部仓库完整测试及一次真实 7072 Codex Agent 请求。
- 本次验收明确不要求修复其他上游错误引发的流式 body decode 二次错误。
- 用户批准变更隔离与错误处理边界：只改外部仓库请求转换器和测试，不改 CC Switch、Profile、数据库、供应商配置、response converter 或流式错误处理；不增加自动探测、配置开关或重试。
- `claude-openai-proxy` 的项目文档给出的完整测试命令为 `uv run python -m pytest`，也支持在现有虚拟环境中运行 `python -m pytest`。
- 现有转换综合测试和 API 入口测试的具体旧断言均为 system 字符串；计划应先增加能红灯复现内容块要求的聚焦测试，再最小修改转换函数，并同步更新这两处既有契约。
- 外部仓库提供 `start.sh`，最终执行 `python -m src.main`；常驻 7072 服务由 `/Users/qihoo/Documents/A_Own/sh/scripts/claude-openai-proxy.sh` 委托该启动脚本并把输出追加到 `/Users/qihoo/Documents/A_Own/sh/logs/claude-openai-proxy.log`。
- 实现计划的自动验证命令应使用 `uv run python -m pytest` 与 `uv run python -m compileall src tests`；真实路由验收需要在用户现有服务管理方式下重启 7072，不能用第二个临时进程抢占端口。

## 文档路径校验

- Findings：`docs/superpowers/findings/2026-07-16-codex-profile-stream-decode-error-findings.md`
- Spec：`docs/superpowers/specs/2026-07-16-codex-profile-stream-decode-error-design.md`
- Plan：`docs/superpowers/plans/2026-07-16-codex-profile-stream-decode-error.md`
- 校验结果：三者均包含固定前缀、正确类型目录、当日日期、kebab-case 主题与规定后缀，可以写入。

## 文档自审结果

- 初次缓存检查发现 Spec 与 Plan 文件末尾各多一行空白，已针对性移除；最终提交前重新执行带失败门控的 `git diff --cached --check`。
- 禁止占位符扫描无命中；Spec 的目标、非目标、转换契约、边界、测试和验收均能映射到 Plan 的三个任务。
- Plan 中使用的类型 `Optional[List[Dict[str, str]]]`、函数名 `merge_system_messages()`、常量 `Constants.CONTENT_TEXT` 与现有源码一致。
- 真实验收依赖的 `/Users/qihoo/Documents/A_Own/claude-openai-proxy/.env` 已确认存在；计划不会在日志或命令输出中打印密钥。
- 提交前按 verification-before-completion 要求重新运行完整文档验证，并按 generate-commit 规范只提交 findings、spec、plan 三个文件。
