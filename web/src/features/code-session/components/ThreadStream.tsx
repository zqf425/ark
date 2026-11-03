import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
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
import { TurnBlock } from './TurnBlock'
import { ApprovalCard } from './ApprovalCard'
import { ChildCompletionCard } from './ErrorRow'
import { UserInputCard } from './UserInputCard'
import { Composer } from './Composer'
import { StreamFooter } from './StreamFooter'
import { ChildRunsPanel } from './ChildRunsPanel'
import { rewindAvailabilityByTurn } from './rewind-flow'
import { startReview } from '../api/turns-api'

/** Pixels of slack that still count as "at the bottom" (caret/last-pixel noise). */
const BOTTOM_THRESHOLD = 32

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
  const toast = useToast()

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

  // The review action starts a system-composed turn; progress then flows
  // through the same event stream as any other turn.
  const [reviewBusy, setReviewBusy] = useState(false)
  const startReviewTurn = async (): Promise<void> => {
    if (!threadId || reviewBusy) return
    setReviewBusy(true)
    try {
      await startReview(threadId)
      toast.push('success', t('canvas.header.reviewStarted'))
    } catch (error) {
      toast.push('error', t('canvas.header.reviewFailed'), describeError(error, translateNow))
    } finally {
      setReviewBusy(false)
    }
  }

  const jumpToTurn = useCallback((turnId: string): void => {
    document.getElementById(`ark-turn-${turnId}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }, [])

  if (!threadId) {
    return (
      <section className="ark-stream ark-stream--empty">
        <div className="ark-stream__emptycall">
          <div className="ark-hero-stage" aria-hidden="true">
            <div className="ark-hero-stage__window">
              <div className="ark-hero-stage__bar">
                <i />
                <i />
                <i />
              </div>
              <div className="ark-hero-stage__canvas">
                <div className="ark-hero-stage__rail">
                  <i />
                  <i />
                  <i />
                </div>
                <div className="ark-hero-stage__lines">
                  <i />
                  <i className="is-wide" />
                  <i className="is-narrow" />
                </div>
              </div>
            </div>
          </div>
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
      <header className={`ark-threadfocus${live ? ' is-live' : ''}`}>
        <div className="ark-threadfocus__identity">
          <h1 title={activeThread?.title ?? ''}>{activeThread?.title || t('canvas.untitled')}</h1>
          <p className={live ? 'is-live' : ''} aria-live="polite">
            {live ? t('canvas.live') : groups.length > 0 ? t('canvas.turnCount', { count: groups.length }) : t('canvas.ready')}
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="ark-threadfocus__review"
          disabled={live || reviewBusy}
          onClick={() => void startReviewTurn()}
        >
          {reviewBusy ? t('canvas.header.reviewBusy') : t('canvas.header.review')}
        </Button>
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

      {groups.length > 1 ? (
        <nav className="ark-jumprail" aria-label={t('canvas.jumpRail.aria')}>
          {groups.map((group, index) => (
            <button
              key={group.turnId}
              type="button"
              title={t('canvas.jumpRail.turn', { index: index + 1 })}
              aria-label={t('canvas.jumpRail.turn', { index: index + 1 })}
              onClick={() => jumpToTurn(group.turnId)}
            />
          ))}
        </nav>
      ) : null}

      {!atBottom ? (
        <button type="button" className="ark-stream__jump" onClick={scrollToBottom}>
          {t('canvas.jumpToLatest')}
        </button>
      ) : null}

      <StreamFooter />
      {/*
        Runtime overrides live inside the composer: model/permission/reasoning
        in its toolbar and workspace/compaction behind the 「会话信息」 chevron,
        so nothing is permanently docked above the input. Keyed by thread so
        switching sessions remounts the textarea: the composer keeps its
        message locally, and an unkeyed one would carry the previous thread's
        unsent draft (ready to be sent to the new one).
      */}
      <div className="ark-composer-stack">
        <Composer key={threadId ?? ''} threadId={threadId} />
      </div>
    </section>
  )
}
