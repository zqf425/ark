import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Button, EmptyState, ErrorBoundary } from '../../../shared/ui'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import {
  approvalCards,
  childCompletionNotices,
  groupByTurn,
  hasLiveItems,
  recentErrors,
  userInputCards,
  type ItemView,
  type TurnGroup,
  type TurnView
} from '../state/selectors'
import { UserBubble, AssistantBubble, ReasoningBlock } from './MessageBubble'
import { ToolCard } from './ToolCard'
import { ChildCompletionCard, ErrorRow, NoticeRow, ToolResultRow } from './ErrorRow'
import { ApprovalCard } from './ApprovalCard'
import { UserInputCard } from './UserInputCard'
import { Composer } from './Composer'
import { StreamFooter } from './StreamFooter'
import { ChildRunsPanel } from './ChildRunsPanel'
import { RewindControl } from './RewindControl'
import { rewindAvailabilityByTurn, type RewindAvailability } from './rewind-flow'
import { SessionRuntimeControls } from './SessionRuntimeControls'

/** Pixels of slack that still count as "at the bottom" (caret/last-pixel noise). */
const BOTTOM_THRESHOLD = 32

/**
 * One row dispatcher. This is the entire "event → UI" decision surface left on
 * the client: pick a component by `item.kind`, no semantic reconstruction.
 *
 * Memoized on the item object: the reducer keeps the identity of every item a
 * fold did not touch, so a streamed token re-renders exactly one row.
 */
const StreamRow = memo(function StreamRow({ item }: { item: ThreadItem }): JSX.Element | null {
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
})

/**
 * One turn block. `group` keeps its identity while the turn's rows and state do
 * (see `groupByTurn`), so a token streaming into the newest turn leaves every
 * earlier turn untouched — no re-render, no boundary re-mount.
 */
const TurnBlock = memo(function TurnBlock({
  threadId,
  group,
  availability
}: {
  threadId: string
  group: TurnGroup
  availability: RewindAvailability
}): JSX.Element {
  const { t } = useI18n()
  return (
    <div className="ark-turn">
      {group.items.map((item) => (
        <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
          <StreamRow item={item} />
        </ErrorBoundary>
      ))}
      <ErrorBoundary label={t('thread.canvas.boundaryRewind')}>
        <RewindControl threadId={threadId} turnId={group.turnId} availability={availability} />
      </ErrorBoundary>
    </div>
  )
})

/**
 * Follow the newest row, event-driven.
 *
 * The previous version rebuilt a dependency string from every item's text
 * length on every render (O(n) per streamed token) and read `scrollHeight` in
 * an effect, forcing a synchronous layout per delta. Content growth is now
 * observed directly: a ResizeObserver coalesces a burst of tokens into at most
 * one scroll write per frame, and no render depends on the row count at all.
 */
function useStickToBottom(threadId: string | null): {
  scrollRef: React.RefObject<HTMLDivElement>
  contentRef: React.RefObject<HTMLDivElement>
  atBottom: boolean
  scrollToBottom: () => void
} {
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const contentRef = useRef<HTMLDivElement | null>(null)
  const atBottomRef = useRef(true)
  const [atBottom, setAtBottom] = useState(true)

  const measure = useCallback(() => {
    const node = scrollRef.current
    if (!node) return
    const next = node.scrollHeight - node.scrollTop - node.clientHeight <= BOTTOM_THRESHOLD
    atBottomRef.current = next
    setAtBottom((prev) => (prev === next ? prev : next))
  }, [])

  const scrollToBottom = useCallback(() => {
    const node = scrollRef.current
    if (!node) return
    node.scrollTop = node.scrollHeight
    atBottomRef.current = true
    setAtBottom(true)
  }, [])

  useEffect(() => {
    const node = scrollRef.current
    if (!node) return undefined
    node.addEventListener('scroll', measure, { passive: true })
    measure()
    return () => node.removeEventListener('scroll', measure)
  }, [measure, threadId])

  useEffect(() => {
    const content = contentRef.current
    if (!content || typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(() => {
      const node = scrollRef.current
      if (!node || !atBottomRef.current) return
      node.scrollTop = node.scrollHeight
    })
    observer.observe(content)
    return () => observer.disconnect()
  }, [threadId])

  // Switching threads lands on the newest row, exactly like the old
  // per-render effect did for a fresh, changed item list.
  useEffect(() => {
    const node = scrollRef.current
    if (!node) return
    node.scrollTop = node.scrollHeight
    atBottomRef.current = true
    setAtBottom(true)
  }, [threadId])

  return { scrollRef, contentRef, atBottom, scrollToBottom }
}

export function ThreadStream({
  threadId,
  onCreateThread
}: {
  threadId: string | null
  onCreateThread?: () => Promise<unknown>
}): JSX.Element {
  const { t } = useI18n()

  // One slice per channel: an event that only appends a delta leaves
  // `approvals`, `userInputs`, `errors` and `childCompletions` untouched, so
  // their subscribers never wake up (see `items-draft.ts`).
  const itemOrder = useCodeSessionStore((s) => s.itemOrder)
  const itemsById = useCodeSessionStore((s) => s.itemsById)
  const turnState = useCodeSessionStore((s) => s.turnState)
  const activeTurnId = useCodeSessionStore((s) => s.activeTurnId)
  const approvalsById = useCodeSessionStore((s) => s.approvals)
  const userInputsById = useCodeSessionStore((s) => s.userInputs)
  const errorLines = useCodeSessionStore((s) => s.errors)
  const completionsById = useCodeSessionStore((s) => s.childCompletions)
  const detailStatus = useCodeSessionStore((s) => s.detailStatus)
  const detailError = useCodeSessionStore((s) => s.detailError)
  const activeThread = useCodeSessionStore((s) => s.threads.find((thread) => thread.thread_id === threadId) ?? null)

  const itemView = useMemo<ItemView>(() => ({ itemOrder, itemsById }), [itemOrder, itemsById])
  const turnView = useMemo<TurnView>(() => ({ itemOrder, itemsById, turnState }), [itemOrder, itemsById, turnState])

  // Group identity is carried across recomputes so memoized turn blocks bail
  // out; the ref is a cache, not state (the result is a pure function of view).
  const previousGroups = useRef<readonly TurnGroup[]>([])
  const groups = useMemo(() => {
    const next = groupByTurn(turnView, previousGroups.current)
    previousGroups.current = next
    return next
  }, [turnView])

  const rewindByTurn = useMemo(
    () => rewindAvailabilityByTurn({ activeTurnId, turnState, itemsById, itemOrder }),
    [activeTurnId, turnState, itemsById, itemOrder]
  )
  const approvals = useMemo(() => approvalCards({ approvals: approvalsById }), [approvalsById])
  const userInputs = useMemo(() => userInputCards({ userInputs: userInputsById }), [userInputsById])
  const errors = useMemo(() => recentErrors(errorLines), [errorLines])
  const completions = useMemo(() => childCompletionNotices({ childCompletions: completionsById }), [completionsById])
  const live = useMemo(() => hasLiveItems(itemView), [itemView])

  const { scrollRef, contentRef, atBottom, scrollToBottom } = useStickToBottom(threadId)

  const resolveApprovalLocally = useCodeSessionStore((s) => s.noteApprovalResolved)
  const answerInputLocally = useCodeSessionStore((s) => s.noteUserInputAnswered)

  if (!threadId) {
    return (
      <section className="ark-stream ark-stream--empty">
        <div className="ark-stream__emptycall">
          <p className="ark-stream__emptykicker" aria-hidden="true">
            {t('canvas.empty.kicker')}
          </p>
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
      </header>
      <div className="ark-stream__scroll" ref={scrollRef}>
        {/* Inner wrapper exists so a single ResizeObserver can see content
            growth; the flex gaps are inherited so the layout is unchanged. */}
        <div className="ark-stream__content" ref={contentRef}>
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
            <TurnBlock
              key={group.turnId}
              threadId={threadId}
              group={group}
              availability={rewindByTurn.get(group.turnId) ?? 'hidden'}
            />
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
      </div>

      {!atBottom ? (
        <button type="button" className="ark-stream__jump" onClick={scrollToBottom}>
          {t('canvas.jumpToLatest')}
        </button>
      ) : null}

      <StreamFooter />
      {/*
        The runtime overrides dock sits with the composer: the user adjusts
        workspace/provider/model/reasoning right where they type. Keyed by
        thread so switching sessions remounts the textarea: the composer keeps
        its message locally, and an unkeyed one would carry the previous
        thread's unsent draft (ready to be sent to the new one).
      */}
      <div className="ark-composer-stack">
        <SessionRuntimeControls threadId={threadId} />
        <Composer key={threadId ?? ''} threadId={threadId} />
      </div>
    </section>
  )
}
