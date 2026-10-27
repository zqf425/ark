# ark-obs 模块设计

## 职责与边界

`ark-obs` 安装一次性 tracing subscriber，并聚合 provider 已报告的用量。`UsageRecorder` 不伪造
cache 命中率；重复初始化必须返回可控结果而非 panic。

审计持久化、平台指标服务和插件配额属于相邻端口/服务的职责。将来扩展时不得在本 crate 再建立
一套与事件日志或插件服务竞争的注册表。
