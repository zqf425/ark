import { useEffect, useMemo, useRef } from 'react'
import { Button, EmptyState, ErrorBoundary } from '../../../shared/ui'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
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
import { rewindAvailabilityByTurn } from './rewind-flow'
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
  const { t } = useI18n()
  const items = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const detailStatus = useCodeSessionStore((s) => s.detailStatus)
  const detailError = useCodeSessionStore((s) => s.detailError)
  const snapshot = useCodeSessionStore((s) => s)
  const activeThread = useCodeSessionStore((s) => s.threads.find((thread) => thread.thread_id === threadId) ?? null)

  const groups = useMemo(() => groupByTurn(snapshot), [snapshot])
  const rewindByTurn = useMemo(() => rewindAvailabilityByTurn(snapshot), [snapshot])
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
          <EmptyState title={t('canvas.empty.title')} hint={t('canvas.empty.hint')} />
          {onCreateThread ? (
            <Button variant="primary" size="sm" onClick={() => void onCreateThread()}>
              {t('canvas.empty.create')}
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
          <h1 title={activeThread?.title ?? ''}>{activeThread?.title || t('canvas.untitled')}</h1>
          <p aria-live="polite">
            {live ? t('canvas.live') : groups.length > 0 ? t('canvas.turnCount', { count: groups.length }) : t('canvas.ready')}
          </p>
        </div>
        <SessionRuntimeControls threadId={threadId} />
      </header>
      <div className="ark-stream__scroll" ref={ref}>
        {detailStatus === 'loading' ? <p className="ark-stream__hint">{t('canvas.detailLoading')}</p> : null}
        {detailStatus === 'error' ? (
          <div className="ark-stream__hint is-error">
            {t('canvas.detailFailed', { error: detailError ?? t('common.unknownError') })}
          </div>
        ) : null}

        {groups.length === 0 && detailStatus === 'ready' ? (
          <EmptyState title={t('canvas.emptyThreadTitle')} hint={t('canvas.emptyThreadHint')} />
        ) : null}

        {groups.map((group) => (
          <div className="ark-turn" key={group.turnId}>
            {group.items.map((item) => (
              <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
                <StreamRow item={item} />
              </ErrorBoundary>
            ))}
            <ErrorBoundary label={t('thread.canvas.boundaryRewind')}>
              <RewindControl
                threadId={threadId}
                turnId={group.turnId}
                availability={rewindByTurn.get(group.turnId) ?? 'hidden'}
              />
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
          {t('canvas.jumpToLatest')}
        </button>
      ) : null}

      <StreamFooter />
      <Composer threadId={threadId} />
    </section>
  )
}
