# ark-provider 模块设计

## 职责与边界

`ark-provider` 管理模型 provider 的配置文档、校验、脱敏视图、默认条目管理、热更新映射、连通性探测和
显式触发的模型目录查询。它依赖 `ark-llm` 把有效条目转为模型客户端，但不实现流协议解析。

API key 只可作为写入输入；读取接口只能返回 `ProviderView` 的存在标记和掩码提示。持久化与内存
快照更新由同一个 store 串行完成，确保下一次 resolve 立即读取一致配置。

默认条目是管理面的一部分：`upsert(entry, make_default)` 可在写入时设默认（禁用条目不可设默认）、
`set_default` 要求目标存在且启用、删除默认条目即清空默认；`default_provider_id` 持久化在
`providers.json`。list 视图同时返回 `active_provider_id`（当前可实际服务的默认条目）。无显式
provider id 的回合经 `resolve(None)` 使用该默认；`resolve_vision(id)` 与 `resolve` 同一选择规则，
只返回显式正向声明的 `supports_vision`（`None` 一律按未声明处理）。

编辑现有 provider 时空 API key、headers、`max_tokens`、`temperature` 均表示未修改并保留存储值；这使
只读脱敏视图不会在普通表单保存时破坏高级配置。显式清空需独立 patch 语义，不得复用空值。

模型目录只在用户点击查询时请求，既可引用已保存条目，也可使用未保存草稿；空草稿密钥/header 会复用
同 id 的已存机密。目录解析出的 `context_window_tokens` 和 `supports_vision` 只有用户选择模型并保存后才
持久化；`max_output_tokens` 仅出现在目录响应中供设置 UI 展示，不进持久化条目。缺失元数据必须保持
unknown，禁止按模型名推断。已禁用条目仍可查询目录，便于在启用前完成配置。目录响应由 `ark-llm`
限时、限体积、排序和去重，provider 层不缓存明文密钥或远端响应。

`headers`、`temperature`、`max_tokens` 不是仅供展示的字段：有效条目必须将它们映射到每个实际模型请求。
目录已知的上下文窗口会在下一父/子回合收窄全局压缩阈值；若用户设置了输出上限，应先预留该输出空间。

## 验证

覆盖 URL、模型 ID、温度、重复 ID、密钥掩码、探测超时、目录错误/大小边界、目录鉴权头、disabled 条目
发现以及失败后旧快照不变。
