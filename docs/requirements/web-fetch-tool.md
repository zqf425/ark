# 内置 web_fetch 工具

> 状态:**已实现**(离线可测部分;真实网络抓取由发布验收承接)。设计节即实现语义,落地情况
> 见文末。前置:整体架构 §3(能力集中治理)与 [ark-http 模块设计](../modules/ark-http.md)。

## 背景与目标

对标 Claude Code / Codex / Gemini CLI 的内置 web fetch 能力:ark 的内置工具表
(bash/read/edit/write/find/grep/ls/request_user_input/delegate_task/load_skill)没有任何网络
工具,agent 无法自行查阅线上文档、报错与 issue。本需求为 agent 增加一个**经治理出口**的
`web_fetch` 内置工具;模型可主动抓取公开网页的文本内容。

非目标(后续需求或待决):

- **web search**:需要外部搜索服务与密钥配置,涉及 provider/凭据模型,单独立项(待决)。
- **主机 allowlist 配置化**:对抓取目标做按部署的白名单需要 ConfigPatch 契约、GUI 设置项与
  i18n,超出本需求最小范围;后续需要时按配置热更新流程立项。
- HTML→Markdown 保真转换、分块长文读取、图片/二进制内容向模型传递。
- 凭据注入的受保护资源抓取(登录态、私有 API)。

## 设计

1. **工具宿主与形状**:新内置工具 `web_fetch`,实现 `ark_ports::Tool`,schema 为
   `{ "url": string }`。它持有注入的 `Arc<dyn HttpCapability>`,与 `delegate_task` 同类:
   不进入零参的 `builtins()`,由组合根经 `ark_tool::web_tools(http)` 注册到**父与子两个工具表**
   (子代理也需要查文档;子表无 delegate_task 的深度约束不受影响)。
2. **出口集中治理**:工具不持有任何自有 HTTP client,只调用注入的 `HttpBus`,逻辑调用方 id 为
   `ark.builtin.web`。组合根在装配时以
   `HostAllowlist::allow_all()`(新增构造器,`parse` 路径仍拒绝裸 `*`,插件清单不可能声明)
   注册该 egress 授权;超时、重试、大小上限、逐跳重定向校验、脱敏日志与诊断审计全部复用
   HttpBus 既有策略(HttpPolicy 默认:HTTPS-only、30s 超时、8 MiB 响应上限、5 跳重定向)。
3. **SSRF 防线在工具层**:调用出口前解析 URL 并解析主机名,拒绝解析到 loopback / 私有 /
   链路本地 / 未指定地址的目标(IP 字面量直接判;域名先经 DNS 解析再判)。`http://` 由
   HttpPolicy 的 `allow_insecure_http=false` 拒绝。已知局限(如实记录):DNS 解析与实际连接
   之间存在 TOCTOU 窗口,与插件路径同级接受;彻底解法(DNS 分层解析/连接时校验)留待后续。
   **重定向(已修复)**:工具向出口请求时固定带 `follow_redirects: Some(false)`,然后自己逐跳跟进,
   每一跳都重新解析主机并重做 SSRF 校验(预算 5 跳,与 `HttpPolicy::max_redirects` 对齐)。不能交给
   ark-http 的 `deliver_hops`:它每跳的 `guard_target` 只校验 scheme 与 allowlist,全 crate 没有
   loopback/私有/链路本地 IP 校验,而 web 身份的 allowlist 是 `allow_all()`,公网页面 302 到
   `https://169.254.169.254/...` 会被跟随。也不能原样返回 3xx:实测一天 12 次 `web_fetch` 死在
   `request returned status 301`,模型随后用整轮手工推导最终 URL。3xx 缺可用 `Location` 时按终态
   渲染状态码;超过预算返回结构化错误。仍存在的局限只有 DNS 解析与实际连接之间的 TOCTOU 窗口
   (与插件路径同级接受)。
4. **沙箱分类**:`TOOL_METAS` 新增 `web_fetch` 行,baseline `ReadOnly`——它不写工作区;
   Strict 自治下仍需人工批准,Unattended 下随全目录放行。注册表、沙箱策略与
   `GET /v1/runtime/tools` 三方继续读同一张表。
5. **响应处理**:仅接受 2xx。`text/html` 经内置文本提取(去 script/style/标签、解码常见实体、
   折叠空白,不引入新依赖);`text/*` 与 `application/json` 原样透传;其余 content-type 判为
   二进制并拒绝。输出与 bash 同规截断(`DEFAULT_LIMITS.max_bash_bytes`,64 KiB),截断时
   注明。非 2xx 返回 `ok=false` 并携带状态码。
6. **失败语义**:任何失败(SSRF 拒绝、出口拒绝、网络错误、非 2xx、二进制)都以结构化工具输出
   返回给模型,不使回合失败。

## 验收

- allowlist:`allow_all()` 放行任意主机;`HostAllowlist::parse(["*"])` 仍被拒绝(插件路径不放松)。
- 工具单测(假 HttpCapability,无网络):私网/loopback/链路本地目标被拒;HTML 文本提取;
  非 2xx;二进制拒绝;64 KiB 截断。
- 装配:父/子工具表都含 `web_fetch`;egress 授权在启动时注册;`GET /v1/runtime/tools` 出现新行。
- 沙箱:三种自治模式下 `web_fetch` 的决策与 ReadOnly 基线一致。
- 真实网络抓取由发布验收承接(本需求只覆盖离线可测路径)。

## 落地情况

设计要点 1–6 全部按批准语义实现(commit 8ab8ca3):

- 工具实现于 `crates/ark-tool/src/exec/tools/web_fetch.rs`,实现 `ark_ports::Tool`,schema
  为 `{ "url": string }`;`TOOL_METAS` 含 `web_fetch` 行,baseline `ReadOnly`
  (`crates/ark-tool/src/domain/meta.rs`)。
- 注册:组合根经 `ark_tool::web_tools(http)` 注册,父工具表(`apps/arkd/src/wiring/tools.rs`
  的 `build_tools`)与子工具表(`build_child_tools`)都含 `web_fetch`,装配测试
  `apps/arkd/src/wiring/tests.rs` 断言两侧均注册。
- 出口治理:组合根以 `ark_http::HostAllowlist::allow_all()` 注册 egress 授权
  (`apps/arkd/src/wiring/mod.rs`),工具自身不持有 HTTP client,逻辑调用方 id 为
  `ark.builtin.web`;`HostAllowlist::parse(["*"])` 仍拒绝,插件清单不可能声明。
- SSRF 防线（首个 URL 与每一跳重定向）、HTML 文本提取、64 KiB 截断、非 2xx/二进制/网络错误的
  结构化失败输出均在工具内实现，单测以假 `HttpCapability` 覆盖私网/loopback/链路本地拒绝、
  文本提取、非 2xx、二进制拒绝、截断，以及跳转链路（跟随 301、拒绝跳往私有地址、超预算、
  缺 `Location`），全部无真实网络依赖。仍存在的局限只有 DNS 解析与实际连接之间的 TOCTOU
  窗口（见设计第 3 点）。
- 真实网络抓取的端到端验证由 [发布收尾与真实环境验收](release-readiness.md) 承接；
  2026-11-02 已在真实网关下完成(见其验收记录):SSRF 结构化拒绝、`text/plain` 全文返回、
  `text/html` 状态透传均通过。已知观察:目标站可能对工具 UA 返回仅含脚本的降级页,提取为空,
  工具当前返回空正文;改进项(空提取返回说明文字)记录于发布验收记录,未实施。
