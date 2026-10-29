# ark-common 模块设计

## 职责与边界

`ark-common` 提供所有 crate 可依赖的稳定基础：错误和 ID newtype、`Usage`、文本编码、
路径归一化及纯文本工具。它不依赖任何内部 crate。

- `ThreadId`、`TurnId`、`ItemId`、`ChildId`、`RunId` 必须以 newtype 传递，禁止跨边界使用裸 `String` 或 `Uuid`。
- `Usage` 是唯一用量结构；命中率只可由 provider 原生 `hit / (hit + miss)` 得出，字段不全时为 `None`。
- 路径归一化的唯一实现为 `ark_common::path::normalize`；其它 crate 和前端不得复制逻辑。
- Windows `canonicalize` 产生的 verbatim 前缀由 `ark_common::path::strip_verbatim` 在归一化入口统一移除；盘符和 UNC 路径均以非 verbatim 形式参与比较。
- 文本写入必须 UTF-8 无 BOM，原子写入由 `write_text_atomic` 完成。

## 当前实现与注意事项

`encoding` 同时包含纯编解码函数和文件读写/原子替换助手。这与“跨 crate utils 应为纯函数”的
早期约束不完全一致；在未完成边界调整前，调用者可复用这些助手，但不得继续向本 crate 增加业务
存储逻辑。详见 `.tmp/design-migration-audit.md` 的建议 A-4。

## 验证

- ID 与 `Usage` 的 serde 往返测试。
- 路径归一化测试向量在原生与 WASM（实现后）构建中逐字一致。
- 原子写入测试必须检查无 BOM、失败隔离和替换行为。
