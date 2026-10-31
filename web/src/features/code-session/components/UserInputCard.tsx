import { useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import { Button, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { answerUserInput } from '../api/user-inputs-api'
import type { PendingUserInput } from '../state/types'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

/**
 * Structured user-input gate. The answer is free text because the runtime's
 * `user_input_requested` payload carries a prompt, not a schema.
 */
export function UserInputCard({
  request,
  onAnswered
}: {
  request: PendingUserInput
  onAnswered: (requestId: string, answer: string) => void
}): JSX.Element {
  const { t } = useI18n()
  const toast = useToast()
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const answered = request.status === 'answered'

  const submit = async (): Promise<void> => {
    const text = value.trim()
    if (!text || busy) return
    setBusy(true)
    try {
      await answerUserInput(request.request_id, text)
      onAnswered(request.request_id, text)
      setValue('')
    } catch (error) {
      toast.push('error', t('canvas.userInput.failed'), describe(error))
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="ark-gate ark-gate--input">
      <header className="ark-gate__head">
        <span className="ark-gate__title">{t('canvas.userInput.title')}</span>
        {answered ? <span className="ark-chip ark-chip--ok">{t('canvas.userInput.answered')}</span> : null}
      </header>
      <p className="ark-gate__body">{request.prompt}</p>
      {answered ? (
        <p className="ark-gate__answer">{request.answer}</p>
      ) : (
        <div className="ark-gate__form">
          <textarea
            className="ark-textarea"
            rows={2}
            value={value}
            placeholder={t('canvas.userInput.placeholder')}
            onChange={(event) => setValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault()
                void submit()
              }
            }}
          />
          <Button variant="primary" size="sm" disabled={!value.trim() || busy} onClick={() => void submit()}>
            {busy ? t('common.submitting') : t('common.submit')}
          </Button>
        </div>
      )}
    </section>
  )
}
