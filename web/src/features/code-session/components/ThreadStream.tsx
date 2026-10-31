import { useEffect, useMemo, useRef } from 'react'
import { Button, EmptyState, ErrorBoundary } from '../../../shared/ui'
import type { ThreadItem } from '../../../shared/contracts'
import { useCodeSessionStore } from '../state/store'
import { approvalCards, childCompletionNotices, groupByTurn, hasLiveItems, recentErrors, userInputCards } from '../state/selectors'
import { UserBubble, AssistantBubble, ReasoningBlock } from './MessageBubble'
import { ToolCard } from './ToolCard'
import { ChildCompletionCard, ErrorRow, NoticeRow, ToolResultRow } from './ErrorRow'
import { ApprovalCard } from './ApprovalCard'
import { UserInputCard } from './UserInputCard'
import { Composer } from './Composer'
import { StreamFooter } from './StreamFooter'
import { ChildRunsPanel } from './ChildRunsPanel'
import { RewindControl } from './RewindControl'
import { SessionRuntimeControls } from './SessionRuntimeControls'

/**
 * One row dispatcher. This is the entire "event → UI" decision surface left on
 * the client: pick a component by `item.kind`, no semantic reconstruction.
 */
function StreamRow({ item }: { item: ThreadItem }): JSX.Element | null {
  switch (item.kind) {
    case 'user_message':
      return <UserBubble item={item} />
    case 'assistant_message':
      return <AssistantBubble item={item} />
    case 'reasoning':
      return <ReasoningBlock item={item} />
    case 'tool_call':
      return <ToolCard item={item} />
    case 'tool_result':
      return <ToolResultRow item={item} />
    case 'error':
      return <ErrorRow item={item} />
    case 'approval':
    case 'user_input':
      // Gates render from live control events below, so a historical gate item
      // would duplicate the card. Keep it out of the stream.
      return null
    case 'notice':
    default:
      return <NoticeRow item={item} />
  }
}

/** Auto-scroll only while the user is already near the bottom. */
function useStickToBottom(dependency: unknown): {
  ref: React.RefObject<HTMLDivElement>
  atBottom: boolean
  scrollToBottom: () => void
} {
  const ref = useRef<HTMLDivElement | null>(null)
  const atBottomRef = useRef(true)

  useEffect(() => {
    const node = ref.current
    if (!node || !atBottomRef.current) return
    node.scrollTop = node.scrollHeight
  }, [dependency])

  const scrollToBottom = (): void => {
    const node = ref.current
    if (!node) return
    node.scrollTop = node.scrollHeight
    atBottomRef.current = true
  }

  return { ref, atBottom: atBottomRef.current, scrollToBottom }
}

export function ThreadStream({
  threadId,
  onCreateThread
}: {
  threadId: string | null
  onCreateThread?: () => Promise<unknown>
}): JSX.Element {
  const items = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const detailStatus = useCodeSessionStore((s) => s.detailStatus)
  const detailError = useCodeSessionStore((s) => s.detailError)
  const snapshot = useCodeSessionStore((s) => s)
  const activeThread = useCodeSessionStore((s) => s.threads.find((thread) => thread.thread_id === threadId) ?? null)

  const groups = useMemo(() => groupByTurn(snapshot), [snapshot])
  const approvals = useMemo(() => approvalCards(snapshot), [snapshot])
  const userInputs = useMemo(() => userInputCards(snapshot), [snapshot])
  const errors = useMemo(() => recentErrors(snapshot), [snapshot])
  const completions = useMemo(() => childCompletionNotices(snapshot), [snapshot])
  const live = useMemo(() => hasLiveItems(snapshot), [snapshot])
  const { ref, atBottom, scrollToBottom } = useStickToBottom(`${items.length}:${completions.length}:${items.map((id) => itemsById[id]?.text?.length ?? 0).join(',')}`)

  const resolveApprovalLocally = useCodeSessionStore((s) => s.noteApprovalResolved)
  const answerInputLocally = useCodeSessionStore((s) => s.noteUserInputAnswered)

  if (!threadId) {
    return (
      <section className="ark-stream ark-stream--empty">
        <div className="ark-stream__emptycall">
          <EmptyState title="开始一个会话" hint="新建或选择一个会话，再描述希望 ark 完成的工作。" />
          {onCreateThread ? (
            <Button variant="primary" size="sm" onClick={() => void onCreateThread()}>
              新建会话
            </Button>
          ) : null}
        </div>
      </section>
    )
  }

  return (
    <section className="ark-stream">
      <header className="ark-threadfocus">
        <div className="ark-threadfocus__identity">
          <h1 title={activeThread?.title ?? ''}>{activeThread?.title || '未命名会话'}</h1>
          <p aria-live="polite">{live ? '正在执行' : groups.length > 0 ? `${groups.length} 个回合` : '准备开始'}</p>
        </div>
        <SessionRuntimeControls threadId={threadId} />
      </header>
      <div className="ark-stream__scroll" ref={ref}>
        {detailStatus === 'loading' ? <p className="ark-stream__hint">正在载入会话…</p> : null}
        {detailStatus === 'error' ? (
          <div className="ark-stream__hint is-error">载入失败：{detailError ?? '未知错误'}</div>
        ) : null}

        {groups.length === 0 && detailStatus === 'ready' ? (
          <EmptyState title="会话为空" hint="在下方输入框发送第一条消息。" />
        ) : null}

        {groups.map((group) => (
          <div className="ark-turn" key={group.turnId}>
            {group.items.map((item) => (
              <ErrorBoundary key={item.item_id} label="该条消息渲染失败">
                <StreamRow item={item} />
              </ErrorBoundary>
            ))}
            <ErrorBoundary label="回退入口渲染失败">
              <RewindControl threadId={threadId} turnId={group.turnId} />
            </ErrorBoundary>
          </div>
        ))}

        {completions.map((notice) => (
          <ChildCompletionCard key={notice.notification_id} notice={notice} />
        ))}
        <ChildRunsPanel threadId={threadId} />

        {approvals.map((approval) => (
          <ApprovalCard key={approval.approval_id} approval={approval} onResolved={resolveApprovalLocally} />
        ))}
        {userInputs.map((request) => (
          <UserInputCard key={request.request_id} request={request} onAnswered={answerInputLocally} />
        ))}

        {errors.length > 0 ? (
          <div className="ark-stream__errors">
            {errors.map((line) => (
              <div className="ark-errorline" key={`${line.seq}-${line.timestamp}`}>
                <code className="ark-errorline__code">{line.code}</code>
                <span className="ark-errorline__message">{line.message}</span>
              </div>
            ))}
          </div>
        ) : null}
      </div>

      {!atBottom ? (
        <button type="button" className="ark-stream__jump" onClick={scrollToBottom}>
          跳到最新
        </button>
      ) : null}

      <StreamFooter />
      <Composer threadId={threadId} />
    </section>
  )
}
