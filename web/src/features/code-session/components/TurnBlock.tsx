/**
 * One turn block: the user's message, then the turn's work process in a single
 * collapsible card, then the final reply.
 *
 * Rows inside the process card keep the real event order (see `splitTurnSections`):
 * intermediate narration, reasoning and tool calls interleave exactly as they
 * happened, and only the last textful assistant message is promoted to the
 * reply bubble — the same rule smart-ark applies.
 *
 * `group` keeps its identity while the turn's rows and state do (see
 * `groupByTurn`), so a token streaming into the newest turn leaves every
 * earlier turn untouched — no re-render, no boundary re-mount. The process
 * card auto-expands while the turn runs and auto-collapses when it settles;
 * a user toggle pins it for the life of the block.
 */
import { memo, useState } from 'react'
import type { ThreadItem } from '../../../shared/contracts'
import { useI18n } from '../../../shared/i18n'
import type { TurnGroup } from '../state/selectors'
import { ErrorBoundary } from '../../../shared/ui'
import { splitTurnSections } from './turn-sections'
import { UserBubble, AssistantBubble, ReasoningBlock } from './MessageBubble'
import { ToolCard } from './ToolCard'
import { ErrorRow, NoticeRow, ToolResultRow } from './ErrorRow'
import { RewindControl } from './RewindControl'
import type { RewindAvailability } from './rewind-flow'

/**
 * One row dispatcher. This is the entire "event → UI" decision surface left on
 * the client: pick a component by `item.kind`, no semantic reconstruction.
 *
 * Memoized on the item object: the reducer keeps the identity of every item a
 * fold did not touch, so a streamed token re-renders exactly one row.
 */
export const StreamRow = memo(function StreamRow({ item }: { item: ThreadItem }): JSX.Element | null {
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
  // in event order. Once settled, the last textful reply is promoted out.
  const { user, process, reply } = splitTurnSections(group.items, live ? 'running' : 'settled')

  // `null` = follow the live state; a user toggle pins the card either way.
  const [pinned, setPinned] = useState<boolean | null>(null)
  const open = pinned ?? live

  return (
    <div className="ark-turn" id={`ark-turn-${group.turnId}`}>
      {user.map((item) => (
        <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
          <StreamRow item={item} />
        </ErrorBoundary>
      ))}
      {process.length > 0 ? (
        <details
          className={`ark-process${live ? ' is-live' : ''}`}
          open={open}
          onToggle={(event) => setPinned(event.currentTarget.open)}
        >
          <summary className="ark-process__head">
            <span className={`ark-dot ${live ? 'ark-dot--live' : 'ark-dot--idle'}`} aria-hidden="true" />
            <span className="ark-process__title">{t('canvas.process.title')}</span>
            <span className="ark-process__meta">{t('canvas.process.count', { count: process.length })}</span>
          </summary>
          <div className="ark-process__body">
            {process.map((item) => (
              <ErrorBoundary key={item.item_id} label={t('thread.canvas.boundaryRow')}>
                <StreamRow item={item} />
              </ErrorBoundary>
            ))}
          </div>
        </details>
      ) : null}
      {reply.map((item) => (
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
