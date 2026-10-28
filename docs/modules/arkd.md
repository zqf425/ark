# arkd 组合根设计

## 职责与边界

`apps/arkd` 是唯一允许了解全部具体实现的组合根。它创建配置、会话存储、模型客户端、工具、子代理
执行器、基础能力、服务器状态和 shutdown supervisor，并把实现注入端口。

`wiring` 仅做显式装配，不能包含业务规则、平台特例或生命周期状态机。新增实现应先落入所属 crate；
只有装配代码才允许在这里引用多个实现 crate。

## 子代理装配

child executor 在此复用同一个 `ark-agent::Reactor`，并桥接 cancel/steer、工具集和会话 writer。
它不能创建第二套 agent framework，也不能让模型指定任意 workspace。

父回合开始前由组合根读取 `SessionStore` 的父事件，调用 `ark-agent` 的纯 inbox 投影并作为动态 system
source 注入 reactor。父回合成功后再追加 completion acknowledgement；写入失败记录告警并保留下一回合
的至少一次投递，不能把已经完成的模型回合改写为失败。

读取使用 completion 生命周期窄查询而非完整历史回放。组合根的完成处理必须保持“注入 → 正常完成才确认 →
下一回合不再注入；失败或中断不确认”的顺序。

## 验证

装配变更至少执行 `cargo check -p arkd`；涉及运行时、存储或并发时执行 workspace 测试，并验证启动/停机
收敛路径。
