# 长期 memory 存储

## 当前实现

ark 为用户显式维护、可跨会话复用的长期偏好或事实提供 JSON 存储。HTTP 表面为：

- `GET /v1/memory`：列表；
- `POST /v1/memory`：新增；
- `PATCH /v1/memory/{id}`：更新；
- `DELETE /v1/memory/{id}`：删除；
- `GET /v1/memory/diagnostics`：返回文件路径、条目数与最后写入时间。

数据保存在 `<data_dir>/memory.json`，原子写入并采用 UTF-8 无 BOM。文件缺失或损坏时以空集合启动，
从而不阻断服务启动；下一次成功写入会建立有效文件。文本不得为空且最大为 16 KiB UTF-8 字节。未知 id
更新或删除返回 `not_found`，校验失败返回 `validation_error`。

## 已批准计划

### 向模型 prompt 注入用户记忆（2026-11-01 批准，首期策略；状态：已实现）

范围：只做"全量注入 + 预算上限"，**不包含**自动抽取、检索排序与用户范围（仍留待决）。
实现位置：`apps/arkd/src/memory_seed.rs`（纯函数 `memory_seed_message` / `seed_for_turn`）与
`apps/arkd/src/wiring.rs` 的 `run_turn`；只读口为 `ark_store::MemoryFileStore::read_entries`。

1. 注入点：组合根构建回合种子时（`apps/arkd/src/wiring.rs` 的 `run_turn`），在 skills 目录
   之后、completion inbox 之前追加**至多一条** system 消息。动态尾部注入，不进入静态
   前缀，不破坏 prompt-cache 前缀稳定性（与 skills 同一约束，见 web/arkd 设计 §7.4.6）。
2. 消息形状：`[user memory]\n- <entry text>`（按 created_at 升序，每行一条，文本内换行
   原样保留并整体缩进）。
3. 预算：全部条目拼接后按 UTF-8 字符数限 4096；超限时**整条丢弃**最新的条目直到放得下，
   并在末尾追加一行 `[user memory truncated: N of M entries shown]`。不以任意字符截断
   单条（半条记忆比没有更危险）。
4. 读取时机：每回合种子构建时读一次 `memory.json`（文件小、量级为用户手工条目数），不做
   进程内缓存——用户刚写入的记忆下一回合立即可见。
5. 失败语义：读取/解析失败**跳过注入**并记 warn 日志，不得让回合失败（memory 是增强，
   不是依赖）。空 store 不注入任何消息。
6. 不新增配置开关：store 本身是用户显式维护面，删除条目即等于关闭；避免出现"配置了开关
   却忘开导致记忆静默失效"的状态。后续若引入自动抽取/排序，再评估开关。
7. 该计划实现后，"向模型 prompt 注入的策略"从待决移除；其余待决项保留。

## 待决

- 是否对损坏的 `memory.json` 保留备份及暴露诊断告警；
- 自动抽取、检索排序、用户范围（多用户/多工作区隔离）的策略；
- tags/source 的上限、索引和搜索语义。
