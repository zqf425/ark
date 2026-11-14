# 审批规则引擎与无人值守自动拒绝

> 状态:**已实现**(2026-11-05,离线可测部分;真实模型端到端由发布验收承接)。对标 Claude Code 的
> permission allow/deny 规则与 "always allow":把"哪些调用可以不问人"从写死的 autonomy 档位
> 升级为可配置规则,并给无人值守回合一个明确的、可配置的拒绝语义,替代"永久停在无人应答的
> gate 上"这一事实上的挂死。

## 背景与目标

当前 sandbox 决策只有三档 autonomy(strict / workspace_write / unattended):
- 用户无法表达"bash 里的 `cargo *` 直接放行,其他仍要审批";
- headless `--review` 只能靠"提示词禁止工具调用"防止无人值守回合停在审批 gate
  (review-preset.md §4 明确记录 auto-deny gate 未实现);IM/定时任务的 `unattended` 覆盖
  依赖 `AllowAllGates`,一切放行,粒度全有或全无。

本需求新增:**审批规则引擎**(按工具与命令模式允许或拒绝)+ **auto-deny**(无人值守场景
不再挂死在 gate 上,而是立即以结构化原因拒绝)。

非目标:正则 matcher(首版用前后缀通配)、per-directory 文件授权、审批规则的学习
("always allow" 的 UI 回写)、GUI 审批规则编辑界面(先经 config.json / PATCH)。

## 设计

### 1. 契约(ark-contract,wire + TS 生成)

```text
ApprovalRule {
  tools: Vec<String>        # 空 = 全部工具
  pattern: String           # 空 = 任意参数;否则为 bash 的 command / 其余工具的规范化 JSON
  action: "allow" | "deny"
}
ApprovalSettings {
  rules: Vec<ApprovalRule>          # 顺序即优先级,首条命中生效
  on_unattended: "allow" | "deny"   # 无人值守回合对 NeedsApproval 调用的处置,默认 deny
}
```

- `ConfigPatch.approvals: Option<ApprovalSettings>`;`RuntimeConfig.approvals`(serde default)。
- 分类 **Hot**:回合启动读快照,下一回合生效;校验:规则数 ≤128、pattern ≤4 KiB、
  tools 每项非空 ≤64 字符、action/on_unattended 仅取枚举值。
- `pattern` 只有一个通配语义:**尾随 `*` 为前缀匹配**,其余为完整等值。刻意不做 glob/regex:
  审批是安全边界,匹配语义必须能被人一眼验证。

### 2. 匹配宿主(唯一实现:`ark-tool::policy::approvals`)

纯函数 `decide(rules, on_unattended, tool, args) -> Option<Verdict>`:

- 工具名大小写敏感等值;`pattern` 为空匹配一切。
- `bash` 取 `command` 字符串匹配;其余工具把参数序列化为稳定 JSON 再匹配(键序由
  serde_json 的 BTreeMap 特性保证)。
- 返回 `None` 表示无规则命中,交回原 sandbox 决策;`allow` 把决策放宽到
  `Unrestricted`(**仅**把 `NeedsApproval` 放宽;只读/写决策不受影响),`deny` 直接改判
  `Deny`。
- `SandboxDecision` 枚举新增 `Deny`:到达工具执行器时产生"被审批规则拒绝"的错误输出,
  与 runner 对 `NeedsApproval` 的防御性拒绝同一位置。

### 3. 策略接线

`ark-tool` 新增 `ApprovalPolicy`:包装既有 `BuiltinSandboxPolicy`(决策顺序不变:
先按 autonomy 得 baseline,再让规则裁决 `NeedsApproval`;规则无法放宽只读/写决策)。
组合根在**每回合**用当前快照构造(规则是 Hot 配置,不进程缓存):
- 父路径:`wiring/turns.rs` 的 `policy` 构造处替换为 `ApprovalPolicy::new(inner, snapshot.approvals.clone())`;
- 子路径:`child.rs` 同样包装(子代理与父同受规则约束,覆盖后的权限仍不得高于父线程)。
启动时的全局 runner(`wiring/mod.rs`)保持 `BuiltinSandboxPolicy`:它只服务"无回合上下文"
的既有路径,规则生效点在回合入口。

### 4. auto-deny(ark-agent 新增 `AutoDenyGates`)

新 `GateBroker` 装饰器,包装任意内层 broker:

- `prepare_approval`:先查 `on_unattended`——`deny`(默认)时不注册 gate,直接返回已决议的
  `ApprovalReply::denied("auto-denied: unattended run with approvals.on_unattended=deny")`;
  `allow` 时透传内层(等价于今天的 `AllowAllGates` 行为)。
- `prepare_user_input` 照旧透传(用户输入不是审批面)。

组合根:`wiring/build` 用 `AutoDenyGates::deny_unattended(InteractiveGates)` 包一层注入
runner;headless `arkd run` 的驱动脚本把 config 的 `approvals.on_unattended` 设为默认 deny
即可让无人值守回合快速失败,而不是把进程挂到 `sse_idle_timeout`。child 路径继续
`AllowAllGates`(子回合的工具决策已在沙箱层收窄,并且父回合可中断子任务)。

### 5. 事件语义

规则改判发生在 reactor 之前(sandbox 层),不改事件词汇:`Deny` 决策的调用仍走
`ToolCallStarted/Finished` 配对,输出为失败结果;auto-deny 复用既有
`approval_requested`+`approval_resolved(approved=false)` 配对(deny 不注册 gate,
但仍按原 ID 规则写事件对,保证 UI 与重放一致)。

## 验收

- `approvals` 模块单测:前缀通配、等值、工具过滤、首条命中优先、allow 只放宽
  NeedsApproval、deny 覆盖一切、非 bash 工具的 JSON 规范化。
- `ark-agent`:`AutoDenyGates` deny 不注册、立即拒绝、内层 allow 透传;reactor 对
  `Deny` 决策产生合成失败结果且事件配对完整。
- `ark-config`:PATCH 校验(越界拒绝、Hot 生效)。
- 契约:`cargo test -p ark-contract --lib ts_export` 后 `web/src/generated` 无漂移。
