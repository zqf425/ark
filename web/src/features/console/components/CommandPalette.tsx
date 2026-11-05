/**
 * ⌘K command palette: actions, thread switching and the inspector toggle.
 *
 * Keyboard: ↑/↓ moves, Enter runs, Esc closes. Focus moves into the input on
 * open and the list uses listbox/option semantics. Every label is catalog copy;
 * every action only moves layout state or selects a thread (no hidden fetches).
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import { useI18n } from '../../../shared/i18n'
import { Icon } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { useViewStore } from '../../workbench/state/view-slice'

type Command = {
  id: string
  icon: string
  label: string
  sub?: string
  run: () => void
}

export function CommandPalette(): JSX.Element | null {
  const { t } = useI18n()
  const open = useViewStore((s) => s.commandOpen)
  const setOpen = useViewStore((s) => s.setCommandOpen)
  const setView = useViewStore((s) => s.setView)
  const threads = useCodeSessionStore((s) => s.threads)
  const activeThreadId = useCodeSessionStore((s) => s.activeThreadId)
  const setActiveThread = useCodeSessionStore((s) => s.setActiveThread)
  const toggleRight = useCodeSessionStore((s) => s.toggleRight)

  const [query, setQuery] = useState('')
  const [index, setIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    if (!open) return
    setQuery('')
    setIndex(0)
    const timer = window.setTimeout(() => inputRef.current?.focus(), 0)
    return () => window.clearTimeout(timer)
  }, [open])

  const actions = useMemo<Command[]>(() => [
    { id: 'new', icon: 'plus', label: t('console.palette.actionNew'), run: () => setView('new') },
    { id: 'review', icon: 'diff', label: t('console.palette.actionReview'), run: () => setView('review') },
    { id: 'workbench', icon: 'chat', label: t('console.palette.actionWorkbench'), run: () => setView('code') },
    { id: 'inspector', icon: 'panel', label: t('console.palette.actionInspector'), run: () => toggleRight() },
    { id: 'settings', icon: 'settings', label: t('console.palette.actionSettings'), run: () => setView('settings') }
  ], [t, setView, toggleRight])

  const threadCommands = useMemo<Command[]>(() => threads
    .filter((thread) => !thread.archived)
    .slice(0, 6)
    .map((thread) => ({
      id: `thread:${thread.thread_id}`,
      icon: 'chat',
      label: thread.title || t('canvas.untitled'),
      sub: thread.thread_id === activeThreadId
        ? t('console.top.running')
        : thread.updated_at.slice(0, 10),
      run: () => {
        setActiveThread(thread.thread_id)
        setView('code')
      }
    })), [threads, activeThreadId, t, setActiveThread, setView])

  const trimmed = query.trim().toLowerCase()
  const matches = (command: Command): boolean =>
    !trimmed || command.label.toLowerCase().includes(trimmed)
  const filteredActions = actions.filter(matches)
  const filteredThreads = threadCommands.filter(matches)
  const total = filteredActions.length + filteredThreads.length
  const activeIndex = total === 0 ? -1 : Math.min(index, total - 1)

  if (!open) return null

  const run = (command: Command): void => {
    command.run()
    setOpen(false)
  }

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setIndex((value) => (total === 0 ? 0 : (Math.min(value, total - 1) + 1) % total))
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      setIndex((value) => (total === 0 ? 0 : (Math.min(value, total - 1) + total - 1) % total))
    } else if (event.key === 'Enter') {
      event.preventDefault()
      const command = [...filteredActions, ...filteredThreads][activeIndex]
      if (command) run(command)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      setOpen(false)
    }
  }

  return (
    <div
      className="console-palette"
      role="presentation"
      onClick={(event) => {
        if (event.target === event.currentTarget) setOpen(false)
      }}
    >
      <div
        className="console-palette__card"
        role="dialog"
        aria-modal="true"
        aria-label={t('console.palette.title')}
      >
        <div className="console-palette__input">
          <Icon name="search" size={17} />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value)
              setIndex(0)
            }}
            onKeyDown={onKeyDown}
            placeholder={t('console.palette.placeholder')}
            aria-label={t('console.palette.placeholder')}
            autoComplete="off"
            spellCheck={false}
          />
          <kbd className="console-kbd">Esc</kbd>
        </div>

        <div className="console-palette__list" role="listbox" aria-label={t('console.palette.title')}>
          {filteredActions.length > 0 ? (
            <div className="console-palette__group">{t('console.palette.groupActions')}</div>
          ) : null}
          {filteredActions.map((command, position) => (
            <div
              key={command.id}
              role="option"
              aria-selected={activeIndex === position}
              className={`console-palette__item${activeIndex === position ? ' is-selected' : ''}`}
              onClick={() => run(command)}
              onMouseEnter={() => setIndex(position)}
            >
              <span className="console-palette__item-icon"><Icon name={command.icon} size={15} /></span>
              <span className="console-palette__item-label">{command.label}</span>
              {command.sub ? <span className="console-palette__item-sub">{command.sub}</span> : null}
            </div>
          ))}

          {filteredThreads.length > 0 ? (
            <div className="console-palette__group">{t('console.palette.groupThreads')}</div>
          ) : null}
          {filteredThreads.map((command, position) => {
            const absolute = filteredActions.length + position
            return (
              <div
                key={command.id}
                role="option"
                aria-selected={activeIndex === absolute}
                className={`console-palette__item${activeIndex === absolute ? ' is-selected' : ''}`}
                onClick={() => run(command)}
                onMouseEnter={() => setIndex(absolute)}
              >
                <span className="console-palette__item-icon"><Icon name={command.icon} size={15} /></span>
                <span className="console-palette__item-label">{command.label}</span>
                {command.sub ? <span className="console-palette__item-sub">{command.sub}</span> : null}
              </div>
            )
          })}

          {total === 0 ? (
            <div className="console-palette__empty">{t('console.palette.empty')}</div>
          ) : null}
        </div>

        <div className="console-palette__foot">
          <span><kbd className="console-kbd">↑</kbd><kbd className="console-kbd">↓</kbd> {t('console.palette.hintNavigate')}</span>
          <span><kbd className="console-kbd">↵</kbd> {t('console.palette.hintOpen')}</span>
          <span className="console-palette__foot-grow" />
          <span><kbd className="console-kbd">Esc</kbd> {t('console.palette.hintClose')}</span>
        </div>
      </div>
    </div>
  )
}