/**
 * One turn block: the user's message, then the turn's work process in a single
 * collapsible card, then the final reply framed as its own answer card.
 *
 * Rows inside the process card keep the real event order (see `splitTurnSections`):
 * intermediate narration, reasoning and tool calls interleave exactly as they
 * happened, and only the last textful assistant message is promoted to the
 * final-reply card — the same rule smart-ark applies.
 *
 * `group` keeps its identity while the turn's rows and state do (see
 * `groupByTurn`), so a token streaming into the newest turn leaves every
 * earlier turn untouched — no re-render, no boundary re-mount. The process
 * card auto-expands while the turn runs and auto-collapses when it settles;
 * a user toggle pins it for the life of the block. The pin writes only from
 * real user toggles: the native `toggle` event also fires for React's own
 * open-state writes (mount, controlled updates), and those echoes are
 * filtered in `useEchoSafeToggle` — without the filter the mount echo pins
 * the card open and a settled turn never collapses.
 */
import { memo, useMemo, useRef, useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import type { TurnGroup } from '../state/selectors'
import { Button, ErrorBoundary, useToast } from '../../../shared/ui'
import { splitTurnSections } from './turn-sections'
import { threadWritePaths } from '../tool-file'
import { useEchoSafeToggle } from './use-echo-safe-toggle'
import { UserBubble, AssistantBubble, FinalReplyCard, ReasoningBlock } from './MessageBubble'
import { ToolCard } from './ToolCard'
import { ErrorRow, NoticeRow, ToolResultRow } from './ErrorRow'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { rewindThread } from '../api/rewind-api'
import { RewindControl } from './RewindControl'
import { executeEditResend, userTextOfTurn } from './rewind-flow'
import type { RewindAvailability } from './rewind-flow'
import { useCodeSessionStore } from '../state/store'

/**
 * The server's own message is the only accurate explanation for a refusal
 * (409 running turn, 404 missing target), so it is shown verbatim after the
 * localized hint — the same rule `RewindControl` applies.
 */
function describe(error: unknown): string {
  return describeError(error, translateNow)
}

/**
 * Stable handler factory: maps a prose chip path to `selectFile` (the
 * inspector's File panel expands on pick, per tool-file-preview.md). A hook
 * here is fine — `StreamRow` is a component — but the whitelist itself is
 * computed once per turn in `TurnBlock`, not per row.
 */
function useOpenWrittenFile(writtenPaths: readonly string[]): (path: string) => void {
  const selectFile = useCodeSessionStore((s) => s.selectFile)
  const pathsRef = useRef(writtenPaths)
  pathsRef.current = writtenPaths
  return (path: string) => {
    // Belt and suspenders: only files this turn actually wrote open. The
    // renderer already gates on the same whitelist; this keeps the store
    // write honest even if a chip slips through with a stale prop.
    if (pathsRef.current.includes(path)) selectFile(path)
  }
}

/**
 * One row dispatcher. This is the entire "event → UI" decision surface left on
 * the client: pick a component by `item.kind`, no semantic reconstruction.
 *
 * Memoized on the item object: the reducer keeps the identity of every item a
 * fold did not touch, so a streamed token re-renders exactly one row. The
 * thread id rides along for the user bubble's skill-mention chips; it is stable
 * per mounted stream, so the memo still holds for streamed tokens.
 */
export const StreamRow = memo(function StreamRow({
  item,
  threadId,
  onFileChip,
  writtenPaths
}: {
  item: ThreadItem
  threadId: string | null
  /** Prose file-chip click handler (requirements/prose-path-links.md), one
   * stable identity per turn, created in TurnBlock. */
  onFileChip: (path: string) => void
  /** This turn's write-path whitelist; the renderer upgrades only these. */
  writtenPaths: readonly string[]
}): JSX.Element | null {
  switch (item.kind) {
    case 'user_message':
      return <UserBubble item={item} threadId={threadId} />
    case 'assistant_message':
      return <AssistantBubble item={item} onFileChip={onFileChip} writablePaths={writtenPaths} />
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
 * Edit-and-resend entry for one settled turn (最小版:
 * requirements/edit-and-resend.md).
 *
 * Rendered beside the rewind entry on turns that have a user message. Same
 * availability rules as rewind (the server 409s a live thread); on success the
 * original user text is placed into the thread's draft so the composer is in
 * edit mode — the user reviews, changes, and sends as a new turn. The old
 * turn's rows are already gone (rewind deleted them), so no duplicate view is
 * possible.
 */
function EditResendControl({
  threadId,
  turnId,
  userText,
  availability
}: {
  threadId: string
  turnId: string
  userText: string
  availability: RewindAvailability
}): JSX.Element | null {
  const { t } = useI18n()
  const { push } = useToast()
  const [pending, setPending] = useState(false)

  const setDraft = useCodeSessionStore((s) => s.setDraft)
  const requestStreamReload = useCodeSessionStore((s) => s.requestStreamReload)
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)

  if (availability === 'hidden') return null

  const busy = availability !== 'ready'
  const run = async (): Promise<void> => {
    setPending(true)
    setCommandError(null)
    await executeEditResend({ threadId, turnId, availability }, userText, {
      rewind: rewindThread,
      onRewound: (_response, draft) => {
        setDraft(threadId, draft)
        requestStreamReload()
        push('success', t('session.editResend.done'))
      },
      onError: (error) => {
        const text = describe(error)
        setCommandError(`${t('session.editResend.failed')}：${text}`)
        push('error', t('session.editResend.failed'), text)
      }
    })
    setPending(false)
  }

  return (
    <div className="ark-rewind ark-edit-resend">
      <Button
        variant="ghost"
        size="sm"
        disabled={busy || pending}
        title={busy ? t('session.rewind.busyHint') : t('session.editResend.entryHint')}
        onClick={() => void run()}
      >
        {pending ? t('session.editResend.preparing') : t('session.editResend.entry')}
      </Button>
    </div>
  )
}

export const TurnBlock = memo(function TurnBlock({
  threadId,
  group,
  availability
}: {
  threadId: string
  group: TurnGroup
  availability: RewindAvailability
}): JSX.Element {
  const { t } = useI18n()
  const live = group.state === 'running'
  // While the turn runs nothing is final: every row stays in the process card
  // in event order. Once settled, the last textful reply is promoted out into
  // its own answer card.
  const { user, process, reply } = splitTurnSections(group.items, live ? 'running' : 'settled')
  // One whitelist per turn (requirements/prose-path-links.md): the exact paths
  // this turn's write/edit calls carried. Recomputed only when the turn's item
  // identity changes — `group` keeps its identity across token streams.
  const writtenPaths = useMemo(() => threadWritePaths(group.items), [group.items])
  // Same handler identity for every row of the turn; Markdown absorbs it.
  const openWritten = useOpenWrittenFile(writtenPaths)

  // `null` = follow the live state; a user toggle pins the card either way.
  // The hook ignores native `toggle` echoes of React's own writes (mount and
  // controlled updates both fire the event), so `pinned` means a real user
  // click — that is what lets the card auto-collapse once the turn settles.
  const { open, onToggle } = useEchoSafeToggle(live)
  const userText = userTextOfTurn(group)

  return (
    <div className="ark-turn" id={`ark-turn-${group.turnId}`}>
      {user.map((item) => (
        <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
          <StreamRow item={item} threadId={threadId} onFileChip={openWritten} writtenPaths={writtenPaths} />
        </ErrorBoundary>
      ))}
      {user.length > 0 && userText !== null ? (
        <ErrorBoundary label={t('thread.canvas.boundaryRewind')}>
          <EditResendControl
            threadId={threadId}
            turnId={group.turnId}
            userText={userText}
            availability={availability}
          />
        </ErrorBoundary>
      ) : null}
      {process.length > 0 ? (
        <details
          className={`ark-process${live ? ' is-live' : ''}`}
          open={open}
          onToggle={onToggle}
        >
          <summary className="ark-process__head">
            <span className={`ark-dot ${live ? 'ark-dot--live' : 'ark-dot--idle'}`} aria-hidden="true" />
            <span className="ark-process__title">{t('canvas.process.title')}</span>
            <span className="ark-process__meta">{t('canvas.process.count', { count: process.length })}</span>
          </summary>
          <div className="ark-process__body">
            {process.map((item) => (
              <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
                <StreamRow item={item} threadId={threadId} onFileChip={openWritten} writtenPaths={writtenPaths} />
              </ErrorBoundary>
            ))}
          </div>
        </details>
      ) : null}
      {reply.map((item) => (
        <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
          <FinalReplyCard item={item} onFileChip={openWritten} writablePaths={writtenPaths} />
        </ErrorBoundary>
      ))}
      <ErrorBoundary label={t('thread.canvas.boundaryRewind')}>
        <RewindControl threadId={threadId} turnId={group.turnId} availability={availability} />
      </ErrorBoundary>
    </div>
  )
})
