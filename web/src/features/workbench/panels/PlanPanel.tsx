import { useEffect, useRef, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { PlanView, TodoEntry, TodoStatus } from '../../../shared/contracts'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { clearGoal, getPlan, updateGoal, updateTodos } from '../api/workbench-api'
import { adoptServerGoal, beginPlanFetch, isLatestPlanRequest, isPlanOwner, resetPlanOwnership, shouldApplyPlanResponse, type PlanOwnership } from './plan-ownership'

function describe(error: unknown): string {
  return describeError(error, translateNow)
}

function nextStatus(status: TodoStatus): TodoStatus {
  if (status === 'pending') return 'in_progress'
  if (status === 'in_progress') return 'completed'
  return 'pending'
}

function todoId(): string {
  return typeof crypto?.randomUUID === 'function' ? crypto.randomUUID() : `todo-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

const NO_OWNER: PlanOwnership = { owner: null, serverGoal: '', generation: 0 }

export function PlanPanel(): JSX.Element {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const workbenchRevision = useCodeSessionStore((state) => state.workbenchRevision)
  const toast = useToast()
  const [plan, setPlan] = useState<PlanView | null>(null)
  const [goal, setGoal] = useState('')
  const [newTodo, setNewTodo] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  /** Which thread the state above belongs to. Bookkeeping, never render input. */
  const ownership = useRef<PlanOwnership>(NO_OWNER)
  /**
   * Writes still in flight, keyed by generation. A set (not a counter) so a
   * completion from a superseded or previous-thread write can only remove its
   * own entry: it can neither unlock a newer write nor leave `busy` latched.
   */
  const pendingWrites = useRef<Set<number>>(new Set())
  const [renderedFor, setRenderedFor] = useState<string | null>(threadId)

  // Switching threads clears everything the previous thread left behind, in the
  // same commit: otherwise B paints one frame of A's plan and goal (and offers
  // to save them into B). The generation bump also invalidates in-flight work.
  if (renderedFor !== threadId) {
    setRenderedFor(threadId)
    ownership.current = resetPlanOwnership(threadId, ownership.current.generation)
    pendingWrites.current = new Set()
    setPlan(null)
    setGoal('')
    setNewTodo('')
    setError(null)
    setBusy(false)
  }

  useEffect(() => {
    if (!threadId) return
    // Claim the thread and supersede the previous request before it starts, so a
    // slower response from an older revision cannot repaint stale data — and so
    // the very first load is not dropped for lack of an owner.
    ownership.current = beginPlanFetch(ownership.current, threadId)
    const generation = ownership.current.generation
    const controller = new AbortController()
    void getPlan(threadId, controller.signal)
      .then((next) => {
        // Accept a response only while its thread still owns the panel and no
        // later request or save has superseded it.
        if (!shouldApplyPlanResponse(ownership.current, threadId, generation)) return
        const lastServerGoal = ownership.current.serverGoal
        const nextServerGoal = next.goal ?? ''
        ownership.current = { owner: threadId, serverGoal: nextServerGoal, generation }
        setPlan(next)
        // An SSE-driven reload must not discard a draft the user is typing.
        setGoal((current) => adoptServerGoal(current, lastServerGoal, nextServerGoal))
        setError(null)
      })
      .catch((reason) => {
        if (controller.signal.aborted) return
        if (!shouldApplyPlanResponse(ownership.current, threadId, generation)) return
        // Keep nothing from the previous thread on screen or writable here.
        ownership.current = { owner: threadId, serverGoal: '', generation }
        setPlan(null)
        setGoal('')
        setNewTodo('')
        setError(describe(reason))
      })
    return () => controller.abort()
  }, [threadId, workbenchRevision])

  if (!threadId) {
    return <EmptyState title={t('workbench.noThreadTitle')} hint={t('workbench.noThreadPlanHint')} />
  }
  if (!plan && !error) return <p className="ark-panel__hint">{t('workbench.plan.loading')}</p>

  /** The panel may write only while it still shows `threadId` and no save superseded it. */
  const canWrite = (generation: number): boolean =>
    isPlanOwner(ownership.current.owner, threadId) && isLatestPlanRequest(ownership.current.generation, generation)

  /** Take the next generation so an in-flight `getPlan` cannot undo a save. */
  const beginWrite = (): number | null => {
    if (!isPlanOwner(ownership.current.owner, threadId)) return null
    ownership.current = { ...ownership.current, generation: ownership.current.generation + 1 }
    pendingWrites.current.add(ownership.current.generation)
    setBusy(true)
    return ownership.current.generation
  }

  /** Release this write's own spinner entry, whether or not its result still counts. */
  const endWrite = (generation: number): void => {
    pendingWrites.current.delete(generation)
    setBusy(pendingWrites.current.size > 0)
  }

  const persistTodos = async (todos: TodoEntry[]): Promise<void> => {
    const generation = beginWrite()
    if (generation === null) return
    try {
      const next = await updateTodos(threadId, { todos })
      if (!canWrite(generation)) return
      ownership.current = { ...ownership.current, serverGoal: next.goal ?? ownership.current.serverGoal }
      setPlan(next)
      setError(null)
    } catch (reason) {
      if (!canWrite(generation)) return
      const message = describe(reason)
      setError(message)
      toast.push('error', t('workbench.plan.saveTodosFailed'), message)
    } finally {
      endWrite(generation)
    }
  }

  const saveGoal = async (): Promise<void> => {
    const text = goal.trim()
    if (!text) return
    const generation = beginWrite()
    if (generation === null) return
    try {
      const next = await updateGoal(threadId, text)
      if (!canWrite(generation)) return
      ownership.current = { ...ownership.current, serverGoal: next.goal ?? '' }
      setPlan(next)
      setGoal(next.goal ?? '')
      setError(null)
    } catch (reason) {
      if (!canWrite(generation)) return
      const message = describe(reason)
      setError(message)
      toast.push('error', t('workbench.plan.saveGoalFailed'), message)
    } finally {
      endWrite(generation)
    }
  }

  const removeGoal = async (): Promise<void> => {
    const generation = beginWrite()
    if (generation === null) return
    try {
      const next = await clearGoal(threadId)
      if (!canWrite(generation)) return
      ownership.current = { ...ownership.current, serverGoal: next.goal ?? '' }
      setPlan(next)
      setGoal('')
      setError(null)
    } catch (reason) {
      if (!canWrite(generation)) return
      const message = describe(reason)
      setError(message)
      toast.push('error', t('workbench.plan.clearGoalFailed'), message)
    } finally {
      endWrite(generation)
    }
  }

  const todos = plan?.todos ?? []
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>{t('workbench.plan.title')}</h2>
        <span>{t('workbench.plan.todoCount', { count: todos.length })}</span>
      </header>
      {error ? <p className="ark-panel__error">{error}</p> : null}

      <label className="ark-panel__label" htmlFor="plan-goal">{t('workbench.plan.goalLabel')}</label>
      <textarea
        id="plan-goal"
        className="ark-textarea"
        rows={3}
        value={goal}
        disabled={busy}
        placeholder={t('workbench.plan.goalPlaceholder')}
        onChange={(event) => setGoal(event.target.value)}
      />
      <div className="ark-panel__actions">
        <Button size="sm" variant="primary" disabled={busy || !goal.trim()} onClick={() => void saveGoal()}>
          {t('workbench.plan.saveGoal')}
        </Button>
        <Button size="sm" disabled={busy || !plan?.goal} onClick={() => void removeGoal()}>
          {t('workbench.plan.clear')}
        </Button>
      </div>

      <div className="ark-plan__list">
        {todos.map((todo) => (
          <div className="ark-plan__todo" key={todo.id}>
            <button type="button" className={`ark-plan__status is-${todo.status}`} disabled={busy} onClick={() => void persistTodos(todos.map((row) => row.id === todo.id ? { ...row, status: nextStatus(row.status) } : row))}>
              {todo.status === 'completed' ? '✓' : todo.status === 'in_progress' ? '•' : '○'}
            </button>
            <span className={todo.status === 'completed' ? 'is-done' : ''}>{todo.title}</span>
            <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void persistTodos(todos.filter((row) => row.id !== todo.id))}>{t('common.remove')}</button>
          </div>
        ))}
      </div>

      <div className="ark-panel__add">
        <input className="ark-field__input" value={newTodo} disabled={busy} placeholder={t('workbench.plan.newTodoPlaceholder')} onChange={(event) => setNewTodo(event.target.value)} onKeyDown={(event) => {
          if (event.key === 'Enter' && newTodo.trim()) {
            void persistTodos([...todos, { id: todoId(), title: newTodo.trim(), status: 'pending' }])
            setNewTodo('')
          }
        }} />
        <Button size="sm" disabled={busy || !newTodo.trim()} onClick={() => {
          void persistTodos([...todos, { id: todoId(), title: newTodo.trim(), status: 'pending' }])
          setNewTodo('')
        }}>{t('workbench.plan.add')}</Button>
      </div>
    </section>
  )
}
