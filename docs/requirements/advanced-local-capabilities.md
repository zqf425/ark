# 附件、PTY 与 Studio 浏览器自动化边界

> 状态：**已批准设计边界，未批准实现**。三项能力须各自补充可验收需求后再进入代码。

## 附件上下文

V1 需定义 `AttachmentId`、元数据/引用、上传/finalize、线程/turn 授权、配额、哈希、MIME+magic-byte
校验、原子落盘、GC 与删除。bytes/base64 不进入 JSONL/SSE；web 不能读取任意路径；agent 只接收已授权的
不可变 ref。多模态模型内容必须由 port/adapter 显式支持，不能由 web/provider 特例拼接。

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
