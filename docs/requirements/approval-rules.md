# 审批规则引擎与无人值守自动拒绝

> 状态:**已实现**(2026-11-05,离线可测部分;真实模型端到端由发布验收承接)。对标 Claude Code 的
> permission allow/deny 规则与 "always allow":把"哪些调用可以不问人"从写死的 autonomy 档位
> 升级为可配置规则,并给无人值守回合一个明确的、可配置的拒绝语义,替代"永久停在无人应答的
> gate 上"这一事实上的挂死。

## 背景与目标

本需求落地前，sandbox 决策只有三档 autonomy(strict / workspace_write / unattended)，用户无法表达
"bash 里的 `cargo *` 直接放行，其他仍要审批"；headless、IM 和自动回合也没有统一的无人值守审批处置。

当前实现包含：**审批规则引擎**（按工具与命令模式允许或拒绝）和 **auto-deny**（无人值守场景在
`on_unattended=deny` 时立即以结构化原因拒绝，不再等待无人应答的 gate）。

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
- 分类 **Hot**：每个父/子回合启动时读取当前配置快照；规则表和 `on_unattended` 处置都从下一回合
  生效，运行中的回合保留其启动快照。校验：规则数 ≤128、pattern ≤4 KiB、tools 每项非空 ≤64 字符、
  action/on_unattended 仅取枚举值。
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

- `prepare_approval`：`deny`（默认）时不注册 gate，直接返回已决议的
  `ApprovalReply::denied("auto-denied: unattended run with approvals.on_unattended=deny")`；
  `allow` 时透传内层 `AllowAllGates`，立即批准。两种处置都不会注册交互 gate。
- `prepare_user_input` 透传内层；无人值守父回合的内层是 `AllowAllGates`，因此会立即返回无用户可用的说明。

组合根在**每个父回合入口**同时读取该回合的配置快照和来源，再选择 gate：

- `serve` 收到的显式 `TurnSource::User` 使用裸 `InteractiveGates`，可由 HTTP/UI 决议；
- headless `arkd run` 的所有回合，以及 `TurnSource::Scheduled`、`TurnSource::Im`、
  `AutoContinuation`、`AutoRecovery`，使用由当前快照 `approvals.on_unattended` 构造的
  `AutoDenyGates<AllowAllGates>`；
- child 回合保持 `AllowAllGates`，但其工具仍受每回合快照构造的 `ApprovalPolicy` 与沙箱约束。

因此规则和无人值守处置都是下一回合 Hot 生效，不在进程启动时缓存，也不改变已经运行的回合。

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
