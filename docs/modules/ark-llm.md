# ark-llm 模块设计

## 职责与边界

`ark-llm` 适配模型 endpoint、流式协议、usage 解析、重试与消息修复，并实现
`ark-ports::ModelClient`。它还规范化显式请求的 provider model catalog；不承载 provider 配置的持久化管理，
该职责属于 `ark-provider`。

## 实现约束

- endpoint format 在入口只路由一次到 provider family；禁止在巨型函数中堆叠格式分支。
- SSE 帧解析、增量拼接、重试、usage 与消息/工具参数修复各有唯一实现。
- 流必须逐块消费，不能为长响应无界缓冲。
- 阶段超时只约束「等响应头」与「等下一帧」的静默，不约束整个回合。默认上限为内置
  `DEFAULT_PHASE_TIMEOUT`（120 秒）；组合根可按 live tuning 的 `model_phase_timeout_secs`
  （0 即使用内置默认）通过 `HttpModelClient::with_timeout` 收窄或放宽，每个回合重建客户端，
  因此该值 Hot 生效。provider 发出终止帧
  （`[DONE]` / `message_stop` / `response.completed`）即视为回合结束：pump 立即收尾，不再等
  EOF，否则保持连接的服务端会把已完成的流误判为超时并触发重复请求。终止帧之后到达的字节不得
  发布。收尾时 `StreamAccumulator::finish()` 补发的 tool-call chunk 必须同样转发到 sink，使
  `ModelResponse.tool_calls` 与事件流一致；未发终止帧就 EOF 的流直接失败，不补发未完成的 tool call。
- `cache_hit_rate` 只能读取原生 hit/miss 后计算，不能由 prompt token 猜测。OpenAI 家族在供应商
  未报原生 miss 字段时，可用 `prompt - hit` 代数补全未命中数（brgroup_bl、Nvidia、openrouter
  实测均只报 `cached_tokens`）；同一 usage 对象把 image/audio token 从 prompt 中拆分单独上报
  时（`prompt_tokens_details` / `input_tokens_details` 内含 `image_tokens` / `audio_tokens`），
  余数可能含不可缓存的计费 token（部分网关把图片排除在 `prompt` 之外），此时仍推导该对但必须
  经 `with_cache_approximate` / `mark_approximate` 打上近似标志，展示层以"≈"呈现——带图请求
  的缓存可见性优先于精确性；显式为零的 image/audio
  计数（如 brgroup_bl 的 `image_tokens: 0`）不构成拆分，推导继续生效且不带标志。
- 传输 client 由调用方注入（`HttpModelClient::new` / `ModelCatalogClient::new`）；组合根注入经
  `ark_http::transport_client` 构建的受治理 client（同一代理/TLS 策略），本 crate 不自建传输。
- model catalog 解析远端显式返回的 context/modalities 元数据，以及 `max_completion_tokens` /
  `max_output_tokens` / `max_tokens` / `top_provider.max_completion_tokens` 中的输出上限
  （`max_output_tokens`，缺失即 unknown）。上下文窗口按 `context_length` / `context_window` /
  `context_window_tokens` / `max_context_length` / `max_model_len` / `limits.context_length` /
  `top_provider.context_length` / `metadata.context_length`，以及 HuggingFace Router 的
  `providers[].context_length` 逐项嗅探；识图按 `supports_vision` / `supports_images`（workbuddy
  网关实测形状）/ `capabilities.vision` / `supports_image_in` 与 `input_modalities` /
  `modalities.input` 中的 `image` 判定。通用 `/models` 只有 id
  时能力字段仍是 unknown（导入对话框另做 256K 兜底，不在本层猜测）。GET 请求 10 秒超时、响应最大 1 MiB、
  模型最多 500 个，错误不得回显可能携带 query secret 的 URL。
- 目录 URL 与聊天 URL 共用同一版本段规则（`config::url::model_catalog_url`）：已带 `v1`/`v2`/`v4`/`V1` 等
  版本段（含 `/api/paas/v4` 这类网关前缀）的 base 直接追加 `models`，`/beta` 与聊天一样提升为 `/v1`。
  禁止在目录侧单独判 `ends_with("/v1")`，否则聊天可用而目录 404。非 2xx 响应必须带
  `upstream_status`，使 401/403 与 404 在设置页可区分。
- provider 配置的 `custom_endpoint` 是整 URL 逐字模式：设置后请求不再追加任何 provider family 路径，
  URL 原样使用（仍受脱敏与错误处理约束）。
- 自定义 provider header 可以补充请求，但不得覆盖 endpoint family 的认证、版本或 content-type 头；POST 的
  `Content-Type: application/json` 仅由 HTTP 请求构造器写入一次，避免严格网关因重复同名头而拒绝解析 JSON。
- assistant 侧 `tool_calls`（`ark_ports::ToolCallRef`）按 endpoint family 序列化为原生 wire 形状：openai chat
  用 `tool_calls` 数组（arguments 为 JSON 字符串），anthropic 用 assistant content 的 `tool_use` block，
  openai responses 在 assistant 项后展开为独立 `function_call` 项。旧 JSON 无该字段时行为不变；
  `repair_messages` 的孤立 tool 结果丢弃规则继续生效。
- 用户消息的 `images`（`ark_ports::ModelImage`，mime + 裸 base64）同样按 family 序列化：openai chat 为
  `content: [{type:"text"},{type:"image_url",image_url:{url}}]` 部分（data URL 由共享的
  `common::data_url` 构造），openai responses 为 `input_text`/`input_image`，anthropic 为
  `image` + `source.type=base64` block。`images` 为空时 wire 形状与纯文本消息逐字节一致。

## 推理强度映射（reasoning_effort）

线程运行时覆盖 `reasoning_effort`（`off|low|medium|high`）经 `ProviderConfig.reasoning_effort` 传入
`ModelRequest`，由各协议 mapper 决定线上形状：Chat Completions 用 `reasoning_effort`（`off` 与未设置
都不发送，API 无强制关闭开关）；Responses API 用 `reasoning: { effort }`（同前省略 `off`）；Anthropic
Messages 无 effort 旋钮，三档映射为固定 thinking 预算（2k/8k/16k）并保证 `max_tokens > budget`（不足
时自动抬到 budget+4096），`off` 发送显式 `thinking: {type: disabled}`——这是唯一能强制关闭已默认开启
思考的协议。

## 重试策略（common/retry.rs，唯一实现）

`HttpModelClient::stream` 在**任何模型输出进入 sink 之前**对瞬时失败重试；已发布过增量的失败
不重试（重放会复制文本）。`published` 状态逐帧累积，role-only、ping 等不产生 chunk 的帧不会
清除它，因此这些静默帧之后的流内错误或 EOF 仍保持终态。默认预算：

- 失败分类：`429` 为 `RateLimited`；`408/409/425/5xx` 与传输错误（连接/超时/请求构造外错误）
  为 `Transient`；其余 4xx 为 `Permanent`，永不重试。
- 默认 `RetryPolicy`：初始请求 + 3 次重试（`max_attempts: 4`），指数退避基数 500ms，
  上限 30s（抖动**前**的基数上限；±25% 抖动后实际可达约 37.5s）；
  `RateLimited` 另有 1s 下限，避免高频打网关。
- **Retry-After 优先**（RFC 9110 §10.2.3）：`429`/`5xx` 响应带 `Retry-After`（秒数或 HTTP 日期）
  时以该值为准，上限 30s（同样为抖动前基数，±25% 抖动后约 37.5s）——错误的网关配置不能把回合拖住几分钟。
  秒数为 0 的 `Retry-After` 视为未提供，回退指数退避而非绕过 1s 限流下限（仅采纳 >0 的
  `retryAfterMs`）。
- **抖动 ±25%**（`jittered`，SplitMix64 确定性伪随机，盐为墙钟纳秒 ⊕ 尝试序号）：并发回合
  撞到同一限流窗口时错开重试，不引入随机数依赖。
- 每次重试前 `tracing::warn!` 记录 provider/model/失败类别/尝试序号/延迟，运行时可在日志
  观察 429 频次；本层无取消令牌，退避期间不可中断（回合中断在 reactor 层生效）。
- 纯函数（`delay`/`should_retry`/`parse_retry_after`/`jittered`）与 HTTP 循环分离，全部单测覆盖。

## 验证

每个协议族以录制 SSE fixture 覆盖正常分块、畸形帧、断流、重试和 usage 缺失；模拟客户端用于
agent 无网络测试。`tests/stream_finalization.rs` 以本地 TCP 服务覆盖两条传输收尾边界：发完
终止帧但保持连接，以及无终止帧直接 EOF 时返回截断错误且不发布未完成工具调用。`HttpModelClient` 的单测宿主在
`service/http/tests/http_tests.rs`（经 `#[path]` 挂回 `http.rs` 的 `tests` 模块），生产文件与
测试文件分别遵守单文件行数上限。另有本地 TCP 回归覆盖“delta → role-only/ping → 错误终态”，
断言只发起一次请求且 sink 不收到重放文本。
