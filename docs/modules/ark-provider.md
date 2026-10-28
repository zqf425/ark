# ark-provider 模块设计

## 职责与边界

`ark-provider` 管理模型 provider 的配置文档、校验、脱敏视图、热更新映射和连通性探测。它依赖
`ark-llm` 把有效条目转为模型客户端，但不实现流协议解析。

API key 只可作为写入输入；读取接口只能返回 `ProviderView` 的存在标记和掩码提示。持久化与内存
快照更新由同一个 store 串行完成，确保下一次 resolve 立即读取一致配置。

编辑现有 provider 时空 API key、headers、`max_tokens`、`temperature` 均表示未修改并保留存储值；这使
只读脱敏视图不会在普通表单保存时破坏高级配置。显式清空需独立 patch 语义，不得复用空值。

## 验证

覆盖 URL、模型 ID、温度、重复 ID、密钥掩码、探测超时以及失败后旧快照不变。
