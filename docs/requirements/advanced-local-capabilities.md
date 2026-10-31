# 附件、PTY 与 Studio 浏览器自动化边界

> 状态：附件基础存储与 HTTP 读取已实现；**附件生命周期 v2 首期已批准待实施**（见下），第二期
> 多模态传递随后实施。PTY 与 Studio 浏览器自动化仍为已批准设计边界，未批准实现。

## 附件上下文

当前实现定义 `AttachmentId`、不可变元数据、25 MiB 单附件上限、原子落盘和诊断/内容读取路由；上传
使用 JSON Base64，上传路由的 body 限额覆盖 Base64 膨胀和 JSON 余量，超限以结构化校验错误返回，字节
不会写入 JSON 元数据、JSONL 或 SSE。服务端生成 UUID 并以其构造文件路径，因此 web 不能读取任意路径。
内容读取以 `nosniff` 和安全文件名的附件下载响应防止用户声明的 MIME 在应用源内被内联执行。上传/finalize
分离、线程/turn 授权、哈希、MIME+magic-byte 校验、GC、删除路由和模型多模态传递仍待决；agent 在这些授权
与 adapter 边界落定前不得把未授权附件传给模型。

### 附件生命周期 v2（2026-11-01 批准，分两期实施）

**第一期：授权链与存储完整性（不含模型传递）**

1. **不做上传/finalize 两阶段**。25 MiB 上限内单步上传足够；两阶段的收益（分片、断点、客户端
   二次确认）当前都不存在，先不为想象中的需求加状态机。待决移除。
2. **哈希**：上传时计算 SHA-256 存入不可变元数据（`sha256` 字段，进 wire 契约）。仅作完整性
   与后续去重基础，首版不做内容寻址存储（路径仍用 UUID，避免迁移）。
3. **MIME+magic-byte 校验**：声明了 `mime_type` 的上传做前 16 字节魔法数一致性校验，覆盖
   png/jpg/gif/webp/pdf 五类常见格式；不匹配返回 422 `mime_mismatch`。无法识别的字节序不
   拒绝（未知格式交由声明的 MIME + 下载响应的 nosniff 防线）。未声明 MIME 的上传跳过校验。
4. **线程/turn 关联与授权**：新增 `POST /v1/threads/{thread_id}/attachments`（body:
   `{attachment_id}`）建立关联，写入线程事件流（新事件 `AttachmentAssociated`，可回放）；
   `TurnRequest` 增加 `attachment_ids`，回合启动时校验"每个 id 都已关联到该线程"，未关联或
   不存在返回 404 `attachment_not_associated`。**模型可见性以关联为准**：未关联的附件即使
   存在也绝不进入任何模型上下文。附件元数据与事件进 TS 生成检查。
5. **删除路由**：`DELETE /v1/attachments/{id}` 物理删除字节与元数据；已关联到线程的附件删除
   前置校验：该 id 出现在任何线程事件流中即拒绝删除（409 `attachment_in_use`），先解绑后删
   ——解绑路由首版不做，用户可新建线程重传。
6. **GC**：首版只做诊断——`GET /v1/attachments/diagnostics` 增加 `orphan_count`（无关联的
   存活天数 >30 的附件数）。自动回收策略留待决。

**第二期：模型多模态传递（依赖第一期关联授权）**

1. 范围仅**图片类**（mime 为 image/* 且魔法数校验通过）：回合种子构建时，把本回合
   `attachment_ids` 指向的图片按 `ModelMessage` 的多模态部分传入。非图片附件以
   `preview_text` 注入文本（字段已存在），不传原始字节。
2. `ark_ports::ModelMessage` 从单一 `content: String` 扩展为支持 text + image_url 部分
   （契约演进，openai_chat 适配器序列化为 `content: [{type:"text"|"image_url",...}]` 数组）；
   只有模型 profile 的 `messageParts` 声明含 `image_url` 才传图，否则降级 preview_text 并在
   回合事件中记录一次降级说明。
3. agent 不读取文件系统：图片字节由组合根在构建种子时读出并以 data URL 注入（组合根已持有
   AttachmentStore）；agent 侧只见消息部分。
4. 大小防线：传入模型的单图在 25 MiB 附件上限内，另设每回合总图片预算 20 MiB，超预算的
   图片按序跳过并记录事件。

两期都覆盖：授权拒绝、跨线程越权（关联校验即边界）、路径/凭据不泄露（下载响应沿用 nosniff）、
重启后关联可回放（事件为权威源）、真实端到端验证；改 wire 时执行 TS 生成检查。

## PTY

PTY 不是 bash 工具的输入框。应新增受 sandbox/approval 约束的 `PtySupervisor` port、session id、输入/resize/
chunk/关闭/取消状态与审计。arkd 拥有进程树、cwd、env snapshot、缓冲、TTL 与回收；web 只消费 typed API。
必须先冻结 Windows/Unix 支持矩阵、ANSI/secret 处理和断线/崩溃清理语义。

## Studio 浏览器自动化

`ark-studio` 当前仅占位。实现前冻结 `runner-event/v1`、sidecar 生命周期、BrowserDriver port、IR/RunBundle、
`Verified<T>` gate、制品/凭据保留与取消 SLA。Node+Playwright sidecar 只能观察/执行，不能拥有模型循环；
Tauri/web 不直接启动 node 或浏览器。

三项后续需求都必须覆盖授权拒绝、跨线程越权、路径/凭据泄露、取消、重启、资源上限、协议兼容和真实端到端
验证；改 wire 时执行 TS 生成检查。
