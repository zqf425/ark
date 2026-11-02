/**
 * Composer: attachments, skill mentions, queueing and steering.
 *
 * Enter sends, Shift+Enter inserts a newline. During a live turn the send
 * button becomes a steering action (追加) and a second action queues the draft
 * locally; queued drafts flush automatically when the turn ends, in order.
 * Message state is local: the runtime owns conversation state, and the user's
 * own message returns as a `ThreadItem`.
 */
import { useEffect, useRef, useState } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
import { useCodeSessionStore } from '../state/store'
import { useTurn } from '../hooks/use-turn'
import { uploadAttachment } from '../api/attachments-api'
import { listSkills } from '../api/skills-api'
import { canEditComposer, canSendToThread } from './composer-availability'
import type { SkillSummary } from '../../../shared/contracts'

const MAX_ROWS = 8
/** Base64 of a file is ~4/3 of its bytes; keep uploads inside the runtime cap. */
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024

type PickedAttachment = { id: string; name: string }

function toBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).split(',')[1] ?? '')
    reader.onerror = () => reject(reader.error ?? new Error('read failed'))
    reader.readAsDataURL(file)
  })
}

export function Composer({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  const draft = useCodeSessionStore((s) => s.sending)
  const set = useCodeSessionStore((s) => s.setSending)
  const streamStatus = useCodeSessionStore((s) => s.streamStatus)
  const commandError = useCodeSessionStore((s) => s.commandError)
  const setCommandError = useCodeSessionStore((s) => s.setCommandError)
  const { send, steer, interrupt, canInterrupt } = useTurn(threadId)
  const inputRef = useRef<HTMLTextAreaElement | null>(null)
  const fileRef = useRef<HTMLInputElement | null>(null)

  // Locally queued drafts: filled by 「排队」 during a live turn, flushed in
  // order when the turn ends (the effect below watches the live transition).
  const [queued, setQueued] = useState<string[]>([])
  const [attachments, setAttachments] = useState<PickedAttachment[]>([])
  const [uploading, setUploading] = useState(false)
  const [skills, setSkills] = useState<SkillSummary[] | null>(null)
  const [skillsOpen, setSkillsOpen] = useState(false)

  const inputDisabled = !canEditComposer(threadId, draft)
  const sendDisabled = !canSendToThread(threadId, draft, streamStatus)
  const notOpen = streamStatus !== 'open'
  const live = canInterrupt

  // A live→idle transition is the queue's drain signal. One message per drain:
  // the turn this triggers ends later and drains the next one the same way.
  const wasLive = useRef(false)
  useEffect(() => {
    if (wasLive.current && !live && queued[0] !== undefined) {
      const first = queued[0]
      // Dequeue only after the send is accepted: a failed drain stays in the
      // queue (with the command error shown) so the text is never lost.
      void send(first).then((ok) => {
        if (!ok) return
        setQueued((current) => {
          const index = current.indexOf(first)
          return index === -1 ? current : [...current.slice(0, index), ...current.slice(index + 1)]
        })
      })
    }
    wasLive.current = live
  }, [live, queued, send])

  const submit = async (): Promise<void> => {
    const node = inputRef.current
    if (!node) return
    if (!canSendToThread(threadId, draft, streamStatus)) return
    const text = node.value
    if (!text.trim()) return
    const ids = attachments.map((picked) => picked.id)
    const ok = live ? await steer(text) : await send(text, ids)
    if (ok) {
      node.value = ''
      node.style.height = 'auto'
      if (!live) setAttachments([])
      setCommandError(null)
      set(false)
    }
  }

  const queueDraft = (): void => {
    const node = inputRef.current
    if (!node) return
    const text = node.value.trim()
    if (!text) return
    setQueued((current) => [...current, text])
    node.value = ''
    node.style.height = 'auto'
  }

  const pickFiles = async (files: FileList | null): Promise<void> => {
    if (!files || files.length === 0) return
    setUploading(true)
    setCommandError(null)
    try {
      for (const file of Array.from(files)) {
        if (file.size > MAX_ATTACHMENT_BYTES) {
          setCommandError(t('canvas.composer.attachmentTooLarge', { name: file.name }))
          continue
        }
        const content = await toBase64(file)
        const uploaded = await uploadAttachment({
          original_filename: file.name,
          mime_type: file.type || undefined,
          content
        })
        setAttachments((current) => [
          ...current,
          { id: uploaded.attachment.id, name: file.name }
        ])
      }
    } catch (error) {
      setCommandError(describeError(error, translateNow))
    } finally {
      setUploading(false)
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  const toggleSkills = async (): Promise<void> => {
    const next = !skillsOpen
    setSkillsOpen(next)
    if (next && skills === null) {
      try {
        const diagnostics = await listSkills()
        setSkills(diagnostics.skills)
      } catch {
        setSkills([])
      }
    }
  }

  const mentionSkill = (id: string): void => {
    const node = inputRef.current
    if (!node) return
    node.value = `${node.value ? `${node.value.trimEnd()} ` : ''}/${id} `
    node.focus()
    setSkillsOpen(false)
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

      {queued.length > 0 ? (
        <div className="ark-composer__queue" role="list" aria-label={t('canvas.composer.queuedAria')}>
          {queued.map((text, index) => (
            <span key={`${index}-${text.slice(0, 12)}`} className="ark-chip" role="listitem">
              {t('canvas.composer.queuedChip', { index: index + 1 })} {text.slice(0, 40)}
              {text.length > 40 ? '…' : ''}
              <button
                type="button"
                className="ark-composer__dismiss"
                aria-label={t('canvas.composer.queuedRemove', { index: index + 1 })}
                onClick={() => setQueued((current) => current.filter((_, at) => at !== index))}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      ) : null}

      <div className="ark-composer__box">
        <div className="ark-composer__lead">
          <span>{live ? t('canvas.composer.steerLead') : t('canvas.composer.describeLead')}</span>
          <span className="ark-composer__keys">
            <kbd>Enter</kbd> {t('canvas.composer.keys')} <kbd>Shift + Enter</kbd> {t('canvas.composer.newline')}
          </span>
        </div>

        {attachments.length > 0 ? (
          <div className="ark-composer__attachments" role="list" aria-label={t('canvas.composer.attachmentsAria')}>
            {attachments.map((picked) => (
              <span key={picked.id} className="ark-chip" role="listitem">
                {picked.name}
                <button
                  type="button"
                  className="ark-composer__dismiss"
                  aria-label={t('canvas.composer.attachmentRemove', { name: picked.name })}
                  onClick={() => setAttachments((current) => current.filter((at) => at.id !== picked.id))}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        ) : null}

        <textarea
          ref={inputRef}
          className="ark-composer__input"
          rows={3}
          placeholder={
            threadId
              ? live
                ? t('canvas.composer.steerPlaceholder')
                : t('canvas.composer.sendPlaceholder')
              : t('canvas.composer.noThreadPlaceholder')
          }
          aria-label={live ? t('canvas.composer.steerAria') : t('canvas.composer.sendAria')}
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

        {skillsOpen ? (
          <div className="ark-composer__skills" role="listbox" aria-label={t('canvas.composer.skillsAria')}>
            {skills === null ? (
              <span className="ark-composer__status">{t('canvas.composer.skillsLoading')}</span>
            ) : skills.length === 0 ? (
              <span className="ark-composer__status">{t('canvas.composer.skillsEmpty')}</span>
            ) : (
              skills.map((skill) => (
                <button
                  key={skill.id}
                  type="button"
                  className="ark-composer__skill"
                  role="option"
                  aria-selected={false}
                  onClick={() => mentionSkill(skill.id)}
                >
                  /{skill.id}
                  <span className="ark-composer__skill-name">{skill.name}</span>
                </button>
              ))
            )}
            <p className="ark-composer__status">{t('canvas.composer.skillsHint')}</p>
          </div>
        ) : null}

        <div className="ark-composer__actions">
          <input
            ref={fileRef}
            type="file"
            multiple
            hidden
            aria-hidden="true"
            onChange={(event) => void pickFiles(event.target.files)}
          />
          <Button
            variant="ghost"
            size="sm"
            disabled={inputDisabled || uploading}
            onClick={() => fileRef.current?.click()}
          >
            {uploading ? t('canvas.composer.attachUploading') : t('canvas.composer.attach')}
          </Button>
          <Button variant="ghost" size="sm" disabled={inputDisabled} onClick={() => void toggleSkills()}>
            {t('canvas.composer.skills')}
          </Button>
          <span className={`ark-composer__status${notOpen ? ' is-warn' : ''}`}>
            {notOpen ? t('canvas.composer.streamDown') : t('canvas.composer.streamUp')}
          </span>
          {live ? (
            <Button variant="default" size="sm" disabled={inputDisabled} onClick={queueDraft}>
              {t('canvas.composer.queue')}
            </Button>
          ) : null}
          {live ? (
            <Button variant="default" size="sm" onClick={() => void interrupt()}>
              {t('canvas.composer.interrupt')}
            </Button>
          ) : null}
          <Button variant="primary" size="sm" disabled={sendDisabled} onClick={() => void submit()}>
            {draft ? t('common.submitting') : live ? t('canvas.composer.steer') : t('canvas.composer.send')}
          </Button>
        </div>
      </div>
    </div>
  )
}
