/**
 * Composer: attachments, skill mentions, queueing and steering.
 *
 * Layout follows the smart-ark Code composer: a 24px frosted shell whose first
 * row is the auto-growing textarea, a hairline-separated toolbar underneath
 * (round attach button, quiet runtime pickers on the left; queue/interrupt and
 * the circular gradient send button on the right). Workspace projects, context
 * structure, manual compaction and the two-level model menu are all toolbar
 * pills; there is no unfolded details panel.
 *
 * Enter sends, Shift+Enter inserts a newline. During a live turn the send
 * button becomes a steering action (追加) and a second action queues the draft
 * locally; queued drafts flush automatically when the turn ends, in order.
 * Message state is local: the runtime owns conversation state, and the user's
 * own message returns as a `ThreadItem`. The unsent draft itself lives in the
 * session store keyed by thread id, so switching threads (which remounts this
 * component) keeps each thread's draft instead of dropping it.
 */
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Button } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
import { useCodeSessionStore } from '../state/store'
import { useTurn } from '../hooks/use-turn'
import { associateAttachment, uploadAttachment } from '../api/attachments-api'
import { listSkills, listThreadSkills, refreshThreadSkills } from '../api/skills-api'
import { canEditComposer, canSendToThread } from './composer-availability'
import { AutonomyPicker, WorkspacePicker, ContextRing, ModelPicker } from './ComposerRuntimeControls'
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

/** Round toolbar glyphs, drawn inline so no icon dependency is added. */
function SendGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <path
        d="M4.5 12 20 4.5 15.5 20l-3.2-6.1L4.5 12Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
    </svg>
  )
}

function StopGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true" focusable="false">
      <rect x="6.5" y="6.5" width="11" height="11" rx="2" fill="currentColor" />
    </svg>
  )
}

export function Composer({ threadId }: { threadId: string | null }): JSX.Element {
  const { t } = useI18n()
  // The unsent draft lives in the store keyed by thread, so switching sessions
  // (which remounts this component) restores each thread's own draft instead
  // of dropping it. `draft` below is the boolean send-in-flight flag.
  const draft = useCodeSessionStore((s) => s.sending)
  const set = useCodeSessionStore((s) => s.setSending)
  const draftText = useCodeSessionStore((s) => (threadId ? (s.drafts[threadId] ?? '') : ''))
  const setDraft = useCodeSessionStore((s) => s.setDraft)
  const clearDraft = useCodeSessionStore((s) => s.clearDraft)
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
  const [skillWarnings, setSkillWarnings] = useState(0)
  /** The round + button opens a small insert menu (attachment / skills). */
  const [menuOpen, setMenuOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!menuOpen) return
    const onDown = (event: MouseEvent): void => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) setMenuOpen(false)
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setMenuOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [menuOpen])

  const inputDisabled = !canEditComposer(threadId, draft)
  const sendDisabled = !canSendToThread(threadId, draft, streamStatus)
  const live = canInterrupt

  // Store-backed textarea: writes keep the per-thread draft in sync, and the
  // height follows the stored content so a restored long draft is not clamped
  // to one row.
  const resize = (node: HTMLTextAreaElement): void => {
    node.style.height = 'auto'
    node.style.height = `${Math.min(node.scrollHeight, MAX_ROWS * 22)}px`
  }

  // A restored multi-line draft (thread switch remount) must size its rows on
  // mount; resize() reads scrollHeight, so this runs after the DOM settles.
  // Typing already resizes through onChange, so only the (re)mount matters.
  useLayoutEffect(() => {
    const node = inputRef.current
    if (node && draftText) resize(node)
  }, [threadId])

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

  const resetInput = (): void => {
    if (threadId) clearDraft(threadId)
  }

  const submit = async (): Promise<void> => {
    if (!canSendToThread(threadId, draft, streamStatus)) return
    const text = draftText
    if (!text.trim()) return
    const ids = attachments.map((picked) => picked.id)
    // The runtime rejects turn starts that carry unassociated attachment ids,
    // so every picked id is linked to the thread right before the turn.
    if (!live && ids.length > 0 && threadId) {
      try {
        for (const id of ids) await associateAttachment(threadId, id)
      } catch (error) {
        setCommandError(describeError(error, translateNow))
        return
      }
    }
    const ok = live ? await steer(text) : await send(text, ids)
    if (ok) {
      // Clear only the text that was actually sent; anything typed during the
      // request stays in the draft and a failure keeps everything.
      if (threadId && useCodeSessionStore.getState().drafts[threadId] === text) clearDraft(threadId)
      if (!live) setAttachments([])
      setCommandError(null)
      set(false)
    }
  }

  const queueDraft = (): void => {
    const text = draftText.trim()
    if (!text) return
    setQueued((current) => [...current, text])
    resetInput()
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

  // Skills are what THIS thread loaded (base + extra roots, with load
  // warnings); without a thread the runtime-wide diagnostics are the fallback.
  const toggleSkills = async (): Promise<void> => {
    const next = !skillsOpen
    setSkillsOpen(next)
    if (next && skills === null) {
      try {
        if (threadId) {
          const view = await listThreadSkills(threadId)
          setSkills([...view.base_skills, ...view.extra_skills])
          setSkillWarnings(view.warnings.length)
        } else {
          const diagnostics = await listSkills()
          setSkills(diagnostics.skills)
        }
      } catch {
        setSkills([])
      }
    }
  }

  const reloadSkills = async (): Promise<void> => {
    if (!threadId) return
    try {
      const view = await refreshThreadSkills(threadId)
      setSkills([...view.base_skills, ...view.extra_skills])
      setSkillWarnings(view.warnings.length)
    } catch {
      // Keep the current list: a failed rescan must not blank the panel.
    }
  }

  const mentionSkill = (id: string): void => {
    const next = draftText ? `${draftText.trimEnd()} /${id} ` : `/${id} `
    if (threadId) setDraft(threadId, next)
    inputRef.current?.focus()
    setSkillsOpen(false)
  }

  const placeholder = threadId
    ? live
      ? t('canvas.composer.steerPlaceholder')
      : t('canvas.composer.sendPlaceholder')
    : t('canvas.composer.noThreadPlaceholder')

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
            {attachments.some((picked) => /\.(png|jpe?g|gif|webp|bmp)$/i.test(picked.name)) ? (
              <span className="ark-composer__status">{t('canvas.composer.imageVisionHint')}</span>
            ) : null}
          </div>
        ) : null}

        <textarea
          ref={inputRef}
          className="ark-composer__input"
          rows={1}
          value={draftText}
          placeholder={placeholder}
          aria-label={live ? t('canvas.composer.steerAria') : t('canvas.composer.sendAria')}
          disabled={inputDisabled}
          onChange={(event) => {
            const node = event.currentTarget
            resize(node)
            if (threadId) setDraft(threadId, node.value)
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
            <div className="ark-composer__skillfoot">
              {skillWarnings > 0 ? (
                <span className="ark-composer__status is-warn">{t('canvas.composer.skillWarnings', { count: skillWarnings })}</span>
              ) : null}
              {threadId ? (
                <button type="button" className="ark-linkbtn" onClick={() => void reloadSkills()}>
                  {t('canvas.composer.skillsRefresh')}
                </button>
              ) : null}
            </div>
            <p className="ark-composer__status">{t('canvas.composer.skillsHint')}</p>
          </div>
        ) : null}

        <div className="ark-composer__actions">
          <div className="ark-composer__tools">
            <input
              ref={fileRef}
              type="file"
              multiple
              hidden
              aria-hidden="true"
              onChange={(event) => void pickFiles(event.target.files)}
            />
            {/* Round + trigger with a small insert menu, like the smart-ark
                composer menu: attachment and skills live behind one button. */}
            <div className="ark-composer__pick" ref={menuRef}>
              <button
                type="button"
                className="ark-composer__round"
                disabled={inputDisabled || live}
                aria-haspopup="menu"
                aria-expanded={menuOpen}
                aria-label={t('canvas.composer.menuAria')}
                title={t('canvas.composer.menuAria')}
                onClick={() => setMenuOpen((value) => !value)}
              >
                {uploading ? (
                  <span className="ark-composer__round-text">…</span>
                ) : (
                  <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" focusable="false">
                    <path
                      d="M12 5v14M5 12h14"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.8"
                      strokeLinecap="round"
                    />
                  </svg>
                )}
              </button>
              {menuOpen ? (
                <div className="ark-composer__popover" role="menu" aria-label={t('canvas.composer.menuAria')}>
                  <button
                    type="button"
                    className="ark-composer__option"
                    role="menuitem"
                    disabled={uploading}
                    title={live ? t('canvas.composer.attachLiveHint') : undefined}
                    onClick={() => {
                      setMenuOpen(false)
                      fileRef.current?.click()
                    }}
                  >
                    {uploading ? t('canvas.composer.attachUploading') : t('canvas.composer.attach')}
                  </button>
                  <button
                    type="button"
                    className="ark-composer__option"
                    role="menuitem"
                    onClick={() => {
                      setMenuOpen(false)
                      void toggleSkills()
                    }}
                  >
                    {t('canvas.composer.skills')}
                  </button>
                </div>
              ) : null}
            </div>
            <AutonomyPicker threadId={threadId} />
          </div>
          <div className="ark-composer__send">
            <WorkspacePicker threadId={threadId} />
            <ContextRing threadId={threadId} />
            <ModelPicker threadId={threadId} />
            {live ? (
              <Button variant="default" size="sm" disabled={inputDisabled} onClick={queueDraft}>
                {t('canvas.composer.queue')}
              </Button>
            ) : null}
            {live ? (
              <button
                type="button"
                className="ark-composer__round ark-composer__round--stop"
                aria-label={t('canvas.composer.interrupt')}
                title={t('canvas.composer.interrupt')}
                onClick={() => void interrupt()}
              >
                <StopGlyph />
              </button>
            ) : null}
            <button
              type="button"
              className="ark-composer__sendbtn"
              disabled={sendDisabled}
              aria-label={live ? t('canvas.composer.steer') : t('canvas.composer.send')}
              title={live ? t('canvas.composer.steer') : t('canvas.composer.send')}
              onClick={() => void submit()}
            >
              {draft ? <span className="ark-composer__round-text">…</span> : <SendGlyph />}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
