import { useRef } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../state/store'
import { useTurn } from '../hooks/use-turn'
import { canEditComposer, canSendToThread } from './composer-availability'

const MAX_ROWS = 8

/**
 * Composer. Enter sends, Shift+Enter inserts a newline, send is disabled while a
 * command is in flight. Message state is local: the runtime owns conversation
 * state, and the user's own message returns as a `ThreadItem`.
 */
export function Composer({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const draft = useCodeSessionStore((s) => s.sending)
  const set = useCodeSessionStore((s) => s.setSending)
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const commandError = useCodeSessionStore((s) => s.commandError)
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const { send, steer, interrupt, canInterrupt } = useTurn(threadId)
  const inputRef = useRef<HTMLTextAreaElement | null>(null)

  const inputDisabled = !canEditComposer(threadId, draft)
  const sendDisabled = !canSendToThread(threadId, draft, streamStatus)
  const notOpen = streamStatus !== 'open'

  const submit = async (): Promise<void> => {
    const node = inputRef.current
    if (!node) return
    if (!canSendToThread(threadId, draft, streamStatus)) return
    const text = node.value
    if (!text.trim()) return
    const ok = canInterrupt ? await steer(text) : await send(text)
    if (ok) {
      node.value = ''
      node.style.height = 'auto'
      setCommandError(null)
      set(false)
    }
  }

  return (
    <div className="ark-composer">
      {commandError ? (
        <div className="ark-composer__error" role="alert">
          <span>{commandError}</span>
          <button type="button" className="ark-composer__dismiss" onClick={() => setCommandError(null)}>
            ×
          </button>
        </div>
      ) : null}

      <div className="ark-composer__box">
        <div className="ark-composer__lead">
          <span>{canInterrupt ? t('canvas.composer.steerLead') : t('canvas.composer.describeLead')}</span>
          <span className="ark-composer__keys">
            <kbd>Enter</kbd> {t('canvas.composer.keys')} <kbd>Shift + Enter</kbd> {t('canvas.composer.newline')}
          </span>
        </div>
        <textarea
          ref={inputRef}
          className="ark-composer__input"
          rows={3}
          placeholder={
            threadId
              ? canInterrupt
                ? t('canvas.composer.steerPlaceholder')
                : t('canvas.composer.sendPlaceholder')
              : t('canvas.composer.noThreadPlaceholder')
          }
          aria-label={canInterrupt ? t('canvas.composer.steerAria') : t('canvas.composer.sendAria')}
          disabled={inputDisabled}
          onChange={(event) => {
            const node = event.currentTarget
            node.style.height = 'auto'
            node.style.height = `${Math.min(node.scrollHeight, MAX_ROWS * 22)}px`
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault()
              void submit()
            }
          }}
        />
        <div className="ark-composer__actions">
          <span className={`ark-composer__status${notOpen ? ' is-warn' : ''}`}>
            {notOpen ? t('canvas.composer.streamDown') : t('canvas.composer.streamUp')}
          </span>
          {canInterrupt ? (
            <Button variant="default" size="sm" onClick={() => void interrupt()}>
              {t('canvas.composer.interrupt')}
            </Button>
          ) : null}
          <Button variant="primary" size="sm" disabled={sendDisabled} onClick={() => void submit()}>
            {draft ? t('common.submitting') : canInterrupt ? t('canvas.composer.steer') : t('canvas.composer.send')}
          </Button>
        </div>
      </div>
    </div>
  )
}
