# ark-provider 模块设计

## 职责与边界

`ark-provider` 管理模型 provider 的配置文档、校验、脱敏视图、默认条目管理、热更新映射、连通性探测和
显式触发的模型目录查询。它依赖 `ark-llm` 把有效条目转为模型客户端，但不实现流协议解析。

API key 在 upsert 路径是写入输入，读取分两个出口：列表/能力/诊断一律走 `ProviderView`，只给
`has_api_key` 与掩码提示；`GET /v1/runtime/providers/{id}` 是编辑专用读取，返回完整
`ProviderEntry`（含已存 API key），仅设置表单使用，使编辑已保存条目时密钥字段不为空。该完整读取端点
位于 `ark-server` 的 protected router：配置 bearer token 时必须认证；未配置 token 时，授权边界是
`arkd` 只监听 loopback，访问本机即视为已授权。这不是多用户密钥隔离，运行时端口不得暴露给不可信本地
进程或代理；provider crate 只提供数据能力，不自行实现第二套鉴权。

生产组合根以 `<root>/config` 构造 store，实际文档为 `<root>/config/providers.json`；provider crate 只使用
调用方传入的目录，不自行推导全局根。持久化与内存快照更新由同一个 store 串行完成，确保下一次 resolve
立即读取一致配置。

默认条目是管理面的一部分：`upsert(entry, make_default)` 可在写入时设默认（禁用条目不可设默认）、
`set_default` 要求目标存在且启用、删除默认条目即清空默认；`default_provider_id` 持久化在
`providers.json`。list 视图同时返回 `active_provider_id`（当前可实际服务的默认条目）。无显式
provider id 的回合经 `resolve(None)` 使用该默认；`resolve_vision(id)` 与 `resolve` 同一选择规则，
只返回显式正向声明的 `supports_vision`（`None` 一律按未声明处理）。

编辑现有 provider 时空 API key、headers、`max_tokens`、`temperature` 均表示未修改并保留存储值；这使
普通表单保存不会破坏高级配置。显式清空需独立 patch 语义，不得复用空值。`models`
刻意不在此列：设置表单持有完整清单（由脱敏视图播种），一次保存表达完整新列表，空列表即清空。
表单会回填已存 api key 后原样提交，因此该保留语义仍是幂等的兜底而非编辑主路径。

每个 provider 除默认 `model` 外还维护一个模型清单 `models`（`ProviderEntry.MODELS_MAX = 64`）：保存时
归一化（trim、去重、去掉与默认 `model` 相同的项），超上限整体事务性拒绝而非静默截断。清单只做
"该供应商可用的模型"登记，回合仍以 `model` 为默认——线程运行时覆盖 `runtime.model` 时由
`resolve_thread_provider` 用覆盖值替换默认模型，因此清单是新模型进入会话选择器的登记表，不是
每回合的解析来源。`ProviderView.models` 是该清单的脱敏镜像。

模型目录只在用户点击导入时请求，既可引用已保存条目，也可使用未保存草稿；空草稿密钥/header 会复用
同 id 的已存机密。目录解析出的 `context_window_tokens` 和 `supports_vision` 只有用户选择模型并保存后才
持久化；`max_output_tokens` 仅出现在目录响应中供设置 UI 展示，不进持久化条目。缺失元数据必须保持
unknown，禁止按模型名推断。已禁用条目仍可查询目录，便于在启用前完成配置。目录响应由 `ark-llm`
限时、限体积、排序和去重，provider 层不缓存明文密钥或远端响应。目录 URL 的版本段由 `ark-llm` 与聊天
请求共用同一规则，provider 层不得自行拼接 `/v1/models`。

清单条目是 `ProviderModelInventoryItem`：除 id 外还可携带该模型各自的 `context_window_tokens` 与
`supports_vision`（None 即 unknown，禁止按名称推断）。反序列化兼容旧的纯字符串数组，旧数据读为
unknown 元数据。线程覆盖模型时经 `resolve_for_model` / `resolve_vision(id, model)` 套用清单里该模型的
元数据：覆盖值等于默认 `model` 时读条目级字段，命中清单项时读该行，覆盖到无清单记录的模型时上下文窗口
置空、识图记为 unknown——绝不把默认模型的窗口或识图标记套到别的模型 id 上。默认 `model` 的元数据存放在
条目级字段（清单归一化会去掉与 `model` 相同的项），不依赖清单。设置页把默认模型与清单模型渲染成完全相同的
行、每行只编辑自己的元数据，正是这一「每行归属」语义在 UI 上的对应；`headers` / `temperature` /
`max_tokens` 是供应商级字段，对所有模型生效，不属于任何一行。

`served-model` 校验：条目带**非空清单**时，`resolve_for_model` 拒绝既不等于默认 `model`、也不在
清单里的覆盖模型，错误文本列出全部可服务模型（默认 + 清单），使「model 与 provider_id 配错对」
在派发前失败并可被模型自我纠正，而不是在 provider 网关上 404。条目**无清单**（从未导入目录）时
保持放行——store 无法区分「目录不可知」与「模型不存在」，强校验会破坏从未导入目录的合法覆盖；
要获得校验，先在设置页导入该供应商的目录。`resolve_for_model` 的签名是
`Result<ProviderConfig>`：`provider_id` 未给且无默认供应商时返回 NotFound（子代理以此回落父模型），
其余失败（具名 provider 不存在/无 key/模型不在清单）都是带诊断的 validation 错误。

`headers`、`temperature`、`max_tokens` 不是仅供展示的字段：有效条目必须将它们映射到每个实际模型请求。
目录已知的上下文窗口在回合内按 80% 比例（`ark_llm::COMPACTION_TRIGGER_RATIO`）推导压缩阈值，若用户设置了
输出上限，应先预留该输出空间；未声明窗口的条目按默认窗口 `ark_llm::DEFAULT_CONTEXT_WINDOW_TOKENS`
（256K，与设置页模型清单的兜底一致）推导，压缩不被禁用。该阈值同时随 `GET /v1/runtime/providers` 的
`compaction_threshold_tokens` 发布给渲染层（仅当完全无有效 provider 时字段缺省，界面隐藏水位）。
运行时调优不再提供全局压缩阈值配置：上下文大小一律归属模型条目。

## 验证

覆盖 URL、模型 ID、温度、重复 ID、密钥掩码、探测超时、目录错误/大小边界、目录鉴权头、disabled 条目
发现以及失败后旧快照不变。`ProviderStore` 的单测宿主在 `service/store/tests/store_tests.rs`
（经 `#[path]` 挂回 `store.rs` 的 `tests` 模块），生产文件与测试文件分别遵守单文件行数上限。
