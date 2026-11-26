# ark-obs 模块设计

## 职责与边界

`ark-obs` 安装一次性 tracing subscriber，并聚合 provider 已报告的用量。`UsageRecorder` 不伪造
cache 命中率；重复初始化必须返回可控结果而非 panic。

`arkd` 使用 `try_init_tracing_with_file` 将同一格式化事件同时写到原有控制台流和
`<root>/log/arkd.log`。文件以 create+append 打开，不截断已有日志；headless 控制台分支仍写 stderr，
所以 stdout 只包含模型输出。文件 writer 在进程内用 mutex 串行，初始化时创建父目录。

当前日志达到 10 MiB 时，writer 在同一个写入调用内将其重命名为同目录的
`arkd-<本地轮转时间 YYYYMMDD-HHMMSS-mmm>[-序号].log`，随后创建新的空 `arkd.log` 继续写入。
历史轮转日志不自动删除；重命名失败时重新打开当前日志并返回写入错误。若 rename 后新 active
open 失败，内部保留 `file=None`；后续 write/rotate 会先重开 active file，并按磁盘实际长度重置
size，因此不会永久停在 `NotConnected`。该窗口实际丢失的是失败写入及其后续日志，不是已经轮转的字节。

审计持久化、平台指标服务和插件配额属于相邻端口/服务的职责。将来扩展时不得在本 crate 再建立
一套与事件日志或插件服务竞争的注册表。
