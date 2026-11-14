# 编辑并重发历史消息(最小版)

> 状态:**已实现**(2026-11-05,web 层;真实浏览器交互验收由发布验收承接)。对标 ChatGPT /
> Claude / Cursor 的 "edit message":点历史里自己发过的消息旁的「编辑重发」,移除该回合及
> 其后的全部内容,并把原文放回输入框,用户修改后发送即重新开始。

## 背景与目标

ark 已有 `POST /v1/threads/{id}/rewind`(物理删除 `turn_started` 起的事件,幂等,web 的
`RewindControl` 已验收)。但"改一句话重问"要用户:手动 rewind → 找回原文 → 复制粘贴到输入框。
本需求把三步合成一步,只做产品入口,不新增服务端能力。

## 设计

1. **零服务端改动**:复用既有 rewind API 与 `rewind-flow` 的可用性规则(同一 409 语义:
   线程有活跃回合时整线程禁用)。不新增契约、不新增路由。
2. **入口**:`TurnBlock` 在用户消息气泡下方、rewind 控件旁边渲染「编辑重发」
   (`EditResendControl`),仅当该回合有非空 user_message 时渲染;可用性
   `ready/busy/hidden` 与 rewind 完全一致(计算复用 `rewindAvailabilityByTurn`)。
3. **行为**(纯编排 `executeEditResend`,与 React 分离可测):
   - 以该回合 `turn_id` 调 rewind;
   - 成功后把 user_message 原文写入该线程的 draft(`runtime-slice.setDraft`),用户在
     输入框内修改、发送即开启新回合;
   - 失败时错误经既有 `setCommandError` + toast 通道展示,原文不动。
4. **明确不做的**:旧回合附件不随草稿回填(新回合从纯文本开始,用户可显式重新附加);
   不保留旧分支(rewind 语义是物理删除,分支树另立需求);steer/排队路径不感知此功能。
5. **i18n**:`session.editResend.*` 五个键,中英文双列,受 catalog-completeness 测试约束。

## 验收

- `rewind-flow` 单测:`executeEditResend` 成功路径带原文回调、失败路径不回调、
  availability 非 ready 直接短路。
- `rewind-i18n` / catalog 测试覆盖新键。
- Vitest 全绿 + tsc + `check:rules`。
