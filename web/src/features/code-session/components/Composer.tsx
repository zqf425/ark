import { useRef } from 'react'
import { Button } from '../../../shared/ui'
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
          <span>{canInterrupt ? '补充给正在执行的会话' : '描述本次会话要完成的工作'}</span>
          <span className="ark-composer__keys"><kbd>Enter</kbd> 发送 <kbd>Shift + Enter</kbd> 换行</span>
        </div>
        <textarea
          ref={inputRef}
          className="ark-composer__input"
          rows={3}
          placeholder={threadId ? (canInterrupt ? '向运行中的回合补充指令（Enter steer）' : '发送消息（Enter 发送，Shift+Enter 换行）') : '先新建或选择一个会话'}
          aria-label={canInterrupt ? '向运行中的回合补充指令' : '发送消息'}
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
            {notOpen ? '事件流未连接' : '事件流已连接'}
          </span>
          {canInterrupt ? (
            <Button variant="default" size="sm" onClick={() => void interrupt()}>
              中断
            </Button>
          ) : null}
          <Button variant="primary" size="sm" disabled={sendDisabled} onClick={() => void submit()}>
            {draft ? '提交中…' : canInterrupt ? 'Steer' : '发送'}
          </Button>
        </div>
      </div>
    </div>
  )
}
