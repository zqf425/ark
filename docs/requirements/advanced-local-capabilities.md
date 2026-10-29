# 附件、PTY 与 Studio 浏览器自动化边界

> 状态：附件基础存储与 HTTP 读取已实现；附件授权、线程/turn 关联与模型多模态传递仍待决。PTY 与
> Studio 浏览器自动化仍为已批准设计边界，未批准实现。

## 附件上下文

当前实现定义 `AttachmentId`、不可变元数据、25 MiB 单附件上限、原子落盘和诊断/内容读取路由；上传
使用 JSON Base64，上传路由的 body 限额覆盖 Base64 膨胀和 JSON 余量，超限以结构化校验错误返回，字节
不会写入 JSON 元数据、JSONL 或 SSE。服务端生成 UUID 并以其构造文件路径，因此 web 不能读取任意路径。
内容读取以 `nosniff` 和安全文件名的附件下载响应防止用户声明的 MIME 在应用源内被内联执行。上传/finalize
分离、线程/turn 授权、哈希、MIME+magic-byte 校验、GC、删除路由和模型多模态传递仍待决；agent 在这些授权
与 adapter 边界落定前不得把未授权附件传给模型。

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
