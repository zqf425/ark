# ark-skills 模块设计

## 职责与边界

`ark-skills` 负责发现 `SKILL.md`、解析 front matter、预算、触发激活和渲染 catalog。它把扫描结果
放在可共享快照中；单个目录或文件无效时返回 warning/诊断，不能使服务进程失败。

Skill 是提示词和工具使用说明资产，不是第二个 agent 或任意代码执行通道。激活结果通过端口和契约
提供给回合构建；路径扩展、长度和数量都受 `SkillsConfig` 约束。

用户显式提及 skill 的 id 或 name 会触发自动激活，即使其 front matter 未声明其它触发器；其余命令、
文本模式和文件类型触发保持既有优先级和评分口径。

组合根的 `load_skill` 工具按 id 从当前扫描快照返回完整指令正文；未命中时返回可恢复错误及当前可用 id，
不会让模型猜测或访问 skill 文件路径。

## 验证

覆盖无效 front matter、重复 slug、目录不可读、超预算裁剪、触发优先级和诊断脱敏。
