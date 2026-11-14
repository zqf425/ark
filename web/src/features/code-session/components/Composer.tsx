/**
 * Composer: attachments, skill mentions, queueing and steering.
 *
 * Layout follows the smart-ark Code composer: a 24px frosted shell whose first
 * row is the auto-growing textarea, a hairline-separated toolbar underneath
 * (round attach button, quiet runtime pickers on the left; the delivery
 * chooser and the circular gradient send button on the right). Workspace
 * projects, context structure, manual compaction and the two-level model menu
 * are all toolbar pills; there is no unfolded details panel.
 *
 * Enter sends, Shift+Enter inserts a newline. While a turn is live the primary
 * control is the interrupt button — until the user starts a draft, at which
 * point it is replaced by the smart-ark running-action pair: a steer/queue
 * delivery picker plus a submit button for the chosen mode. Queued drafts
 * flush automatically when the turn ends, in order. Message state is local:
 * the runtime owns conversation state, and the user's own message returns as
 * a `ThreadItem`. The unsent draft itself lives in the session store keyed by
 * thread id, so switching threads (which remounts this component) keeps each
 * thread's draft instead of dropping it.
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { translateNow } from '../../../shared/i18n/current'
import { describeError } from '../../../shared/api/error-describe'
import { useCodeSessionStore } from '../state/store'
import { useTurn } from '../hooks/use-turn'
import { useRuntimeSkills, useThreadSkills } from '../hooks/use-thread-skills'
import { associateAttachment, uploadAttachment } from '../api/attachments-api'
import { canEditComposer, canSendToThread } from './composer-availability'
import { filterSkills, mentionTokenAt, type MentionToken } from './skill-mention'
import { AutonomyPicker, WorkspacePicker, ContextRing, ModelPicker } from './ComposerRuntimeControls'
import { SkillPalette } from './SkillPalette'
import { CopyButton } from '../../../shared/ui/copy'
const MAX_ROWS = 8
/** Base64 of a file is ~4/3 of its bytes; keep uploads inside the runtime cap. */
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024

type PickedAttachment = { id: string; name: string }

/** Where a draft goes while a turn is live: steer the current turn or hold it. */
type RunningDeliveryMode = 'steer' | 'queue'

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

/** Delivery-mode glyphs for the running-action picker (fork = steer). */
function SteerGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" focusable="false">
      <circle cx="6" cy="5.5" r="2.1" fill="none" stroke="currentColor" strokeWidth="1.8" />
      <circle cx="6" cy="18.5" r="2.1" fill="none" stroke="currentColor" strokeWidth="1.8" />
      <circle cx="18" cy="5.5" r="2.1" fill="none" stroke="currentColor" strokeWidth="1.8" />
      <path d="M6 7.6v8.8M18 7.6v2.2c0 1.6-1.2 2.4-2.8 2.4H8.8" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  )
}

/** Delivery-mode glyphs for the running-action picker (list = queue). */
function QueueGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true" focusable="false">
      <path d="M9 6.5h11M9 12h11M9 17.5h11" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" />
      <circle cx="4.4" cy="6.5" r="1.4" fill="currentColor" />
      <circle cx="4.4" cy="12" r="1.4" fill="currentColor" />
      <circle cx="4.4" cy="17.5" r="1.4" fill="currentColor" />
    </svg>
  )
}

function CaretGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="10" height="10" aria-hidden="true" focusable="false">
      <path d="m6 9.5 6 6 6-6" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

/** A thin arc that spins; the busy-state stand-in for the send glyph. */
function SpinnerGlyph(): JSX.Element {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true" focusable="false" className="ark-composer__spinner">
      <path d="M12 3.5a8.5 8.5 0 1 1-8.5 8.5" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" />
    </svg>
  )
}

/**
 * The steer/queue chooser shown next to the send button while a turn is live
 * and the user has a draft. Its label doubles as the submit button's label, so
 * the two always name the same delivery channel. Owns only the open state;
 * the mode itself lives in the Composer so Enter submits through it too.
 */
function DeliveryChooser({
  mode,
  onModeChange
}: {
  mode: RunningDeliveryMode
  onModeChange: (mode: RunningDeliveryMode) => void
}): JSX.Element {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false)
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const label = mode === 'steer' ? t('canvas.composer.steer') : t('canvas.composer.queueSend')

  return (
    <div className="ark-composer__delivery" ref={rootRef}>
      <button
        type="button"
        className="ark-composer__delivery-trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t('canvas.composer.deliveryAria')}
        title={t('canvas.composer.deliveryAria')}
        onClick={() => setOpen((value) => !value)}
      >
        {mode === 'steer' ? <SteerGlyph /> : <QueueGlyph />}
        <span className="ark-composer__delivery-label">{label}</span>
        <CaretGlyph />
      </button>
      {open ? (
        <div className="ark-composer__popover ark-composer__delivery-menu" role="menu">
          <button
            type="button"
            className="ark-composer__delivery-option"
            role="menuitemradio"
            aria-checked={mode === 'steer'}
            onClick={() => {
              onModeChange('steer')
              setOpen(false)
            }}
          >
            <SteerGlyph />
            <span className="ark-composer__delivery-text">
              <span className="ark-composer__delivery-title">{t('canvas.composer.steer')}</span>
              <span className="ark-composer__delivery-desc">{t('canvas.composer.steerDesc')}</span>
            </span>
            {mode === 'steer' ? <span className="ark-composer__delivery-check">✓</span> : null}
          </button>
          <button
            type="button"
            className="ark-composer__delivery-option"
            role="menuitemradio"
            aria-checked={mode === 'queue'}
            onClick={() => {
              onModeChange('queue')
              setOpen(false)
            }}
          >
            <QueueGlyph />
            <span className="ark-composer__delivery-text">
              <span className="ark-composer__delivery-title">{t('canvas.composer.queueSend')}</span>
              <span className="ark-composer__delivery-desc">{t('canvas.composer.queueSendDesc')}</span>
            </span>
            {mode === 'queue' ? <span className="ark-composer__delivery-check">✓</span> : null}
          </button>
        </div>
      ) : null}
    </div>
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
  // Running-action delivery mode, like the smart-ark composer: choosing a mode
  // never submits the draft; it is applied on Enter or the send button. Reset
  // to steer when the draft empties so a fresh draft starts as a steer.
  const [deliveryMode, setDeliveryMode] = useState<RunningDeliveryMode>('steer')
  const [attachments, setAttachments] = useState<PickedAttachment[]>([])
  const [uploading, setUploading] = useState(false)
  // The loaded skill set is shared with the message canvas (mention chips read
  // the same cache), so the palette and the bubbles never disagree.
  const threadSkillView = useThreadSkills(threadId)
  const runtimeSkills = useRuntimeSkills()
  const skills = threadId ? threadSkillView.skills : runtimeSkills
  const skillWarnings = threadId ? threadSkillView.warnings : 0
  const [skillsOpen, setSkillsOpen] = useState(false)
  /**
   * Slash invocation: the mention token under the caret (`/kun-new…` with the
   * caret inside it). Non-null summons the palette in token mode; the draft
   * textarea stays the input, so the palette renders without its own filter.
   */
  const [token, setToken] = useState<MentionToken | null>(null)
  const [tokenActive, setTokenActive] = useState(0)
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
  // Known skill count for the insert-menu badge; loading state hides it.
  const skillCount = skills?.length ?? 0
  // The slash-invocation shortlist: the loaded set filtered by the typed token.
  // An empty result keeps the palette visible with the "no match" row, like the
  // mainstream canvases' slash menu.
  const tokenList = useMemo(
    () => (token && skills ? filterSkills(skills, token.query) : []),
    [token, skills]
  )
  // The steer/queue chooser only exists while the user actually has a draft:
  // an empty running composer is pure stop, matching the smart-ark composer.
  const showDeliveryChooser = live && !sendDisabled && draftText.trim().length > 0

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

  // The delivery chooser is bound to the running state: a fresh turn defaults
  // back to steer, like the smart-ark composer's reset.
  useEffect(() => {
    if (!live) setDeliveryMode('steer')
  }, [live])

  // Draft emptied (sent, queued or cleared) → next draft defaults to steer.
  useEffect(() => {
    if (!draftText.trim()) setDeliveryMode('steer')
  }, [draftText])

  // The slash-invocation palette is caret-bound: a draft that no longer holds
  // the token (sent, cleared, thread switch) must drop it, and the highlight
  // resets so a freshly typed `/` highlights the first row again.
  useEffect(() => {
    if (!draftText.trim()) setToken(null)
  }, [draftText])
  useEffect(() => {
    setToken(null)
    setTokenActive(0)
  }, [threadId])

  const resetInput = (): void => {
    if (threadId) clearDraft(threadId)
  }

  /**
   * The primary action, shared by Enter and the send button. During a live
   * turn the delivery picker decides the channel (smart-ark's running-action
   * pair): steer injects into the current turn, queue holds the draft for the
   * automatic end-of-turn drain.
   */
  const submit = async (): Promise<void> => {
    if (!canSendToThread(threadId, draft, streamStatus)) return
    const text = draftText
    if (!text.trim()) return
    if (live && deliveryMode === 'queue') {
      queueDraft()
      return
    }
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
  // warnings), cached module-level in `useThreadSkills`; `reload` forces the
  // rescan endpoint so the palette can refresh on demand.
  const toggleSkills = async (): Promise<void> => {
    // The two palette modes are mutually exclusive: opening the menu palette
    // closes the slash invocation and vice versa (the menu row checks below).
    setToken(null)
    setSkillsOpen((current) => !current)
  }

  const reloadSkills = (): void => {
    threadSkillView.reload()
  }

  const mentionSkill = (id: string): void => {
    const next = draftText ? `${draftText.trimEnd()} /${id} ` : `/${id} `
    if (threadId) setDraft(threadId, next)
    inputRef.current?.focus()
    setSkillsOpen(false)
  }

  /**
   * Replace the in-progress token under the caret with the full mention, or
   * insert at the caret when there is no token (menu mode). One landing spot
   * for both palette modes, so the caret/lead-space rules stay in one place.
   */
  const applySkill = (id: string): void => {
    const node = inputRef.current
    // Closing the palette is part of the pick: the token below is about to be
    // replaced, and a stale one would keep the palette open after re-render.
    setToken(null)
    setSkillsOpen(false)
    if (!node || !threadId) {
      mentionSkill(id)
      return
    }
    const mention = `/${id} `
    const start = token ? token.start : (node.selectionStart ?? draftText.length)
    const end = token ? token.end : (node.selectionEnd ?? draftText.length)
    const before = draftText.slice(0, start)
    const after = draftText.slice(end)
    // A mention is a word: keep one space before it when gluing onto text.
    const lead = before && !/\s$/.test(before) ? ' ' : ''
    const next = `${before}${lead}${mention}${after}`
    setDraft(threadId, next)
    const caret = before.length + lead.length + mention.length
    // The value lands through the store → prop cycle, so the caret restore
    // waits one frame; without it the cursor jumps to the end of the draft.
    requestAnimationFrame(() => {
      const fresh = inputRef.current
      if (fresh) fresh.setSelectionRange(caret, caret)
    })
    node.focus()
  }

  const placeholder = threadId
    ? live
      ? t('canvas.composer.steerPlaceholder')
      : t('canvas.composer.sendPlaceholder')
    : t('canvas.composer.noThreadPlaceholder')

  // Primary-action label follows the delivery mode during a live turn, like
  // smart-ark: the button and Enter both submit through the chosen channel.
  const primaryActionLabel = live
    ? deliveryMode === 'steer'
      ? t('canvas.composer.steer')
      : t('canvas.composer.queueSend')
    : t('canvas.composer.send')

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
              <CopyButton
                text={text}
                label={t('canvas.copy.queued')}
                copiedLabel={t('canvas.copy.copied')}
                className="ark-chip__copy"
              />
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
            // Track the mention token under the caret: typing `/` opens the
            // palette, whitespace/word-boundary changes close it again.
            const nextToken = mentionTokenAt(node.value, node.selectionStart ?? node.value.length)
            setToken(nextToken)
            setTokenActive(0)
          }}
          onKeyDown={(event) => {
            // Token mode: the palette rides the draft, so navigation keys act
            // on it first. Only intercept while the palette has rows.
            if (token && tokenList.length > 0) {
              if (event.key === 'ArrowDown' && !event.shiftKey) {
                event.preventDefault()
                setTokenActive((current) => (current + 1) % tokenList.length)
                return
              }
              if (event.key === 'ArrowUp' && !event.shiftKey) {
                event.preventDefault()
                setTokenActive((current) => (current - 1 + tokenList.length) % tokenList.length)
                return
              }
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault()
                const picked = tokenList[Math.min(tokenActive, tokenList.length - 1)]
                if (picked) applySkill(picked.id)
                return
              }
              if (event.key === 'Escape') {
                event.preventDefault()
                setToken(null)
                return
              }
            }
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault()
              void submit()
            }
          }}
        />

        {/* Draft copy: only with text in it, so an empty composer stays clean. */}
        {draftText.trim() ? (
          <CopyButton
            text={draftText}
            label={t('canvas.copy.draft')}
            copiedLabel={t('canvas.copy.copied')}
            className="ark-composer__copy"
          />
        ) : null}

        {/* Menu mode: opened from the + menu, carries its own filter. */}
        {skillsOpen && !token ? (
          <SkillPalette
            skills={skills}
            warnings={skillWarnings}
            canRefresh={Boolean(threadId)}
            mode="menu"
            onPick={applySkill}
            onRefresh={reloadSkills}
          />
        ) : null}

        {/* Token mode: summoned by typing `/`; the draft is the query. */}
        {!skillsOpen && token ? (
          <SkillPalette
            skills={skills}
            warnings={skillWarnings}
            canRefresh={false}
            mode="token"
            query={token.query}
            activeIndex={tokenActive}
            onActiveIndexChange={setTokenActive}
            onPick={applySkill}
            onRefresh={reloadSkills}
          />
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
                    <span className="ark-composer__option-row">
                      {t('canvas.composer.skills')}
                      {skillCount > 0 ? <span className="ark-composer__option-badge">{skillCount}</span> : null}
                    </span>
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
            {/* Running state, mirroring the smart-ark composer: the rightmost
                control has one job at a time — interrupt while the draft is
                empty, submit (with a steer/queue chooser) once the user types. */}
            {showDeliveryChooser ? <DeliveryChooser mode={deliveryMode} onModeChange={setDeliveryMode} /> : null}
            {live && !showDeliveryChooser ? (
              <button
                type="button"
                className="ark-composer__round ark-composer__round--stop"
                aria-label={t('canvas.composer.interrupt')}
                title={t('canvas.composer.interrupt')}
                onClick={() => void interrupt()}
              >
                <StopGlyph />
              </button>
            ) : (
              <button
                type="button"
                className="ark-composer__sendbtn"
                disabled={sendDisabled}
                aria-label={primaryActionLabel}
                title={primaryActionLabel}
                onClick={() => void submit()}
              >
                {/* Spinner only for the in-flight request; the streaming turn
                    itself is signalled by the interrupt button above. */}
                {draft ? <SpinnerGlyph /> : <SendGlyph />}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
