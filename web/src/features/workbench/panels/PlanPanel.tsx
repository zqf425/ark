import { useEffect, useState } from 'react'
import { describeError } from '../../../shared/api/error-describe'
import { translateNow } from '../../../shared/i18n/current'
import type { PlanView, TodoEntry, TodoStatus } from '../../../shared/contracts'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useI18n } from '../../../shared/i18n'
import { useCodeSessionStore } from '../../code-session/state/store'
import { clearGoal, getPlan, updateGoal, updateTodos } from '../api/workbench-api'

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

export function PlanPanel(): JSX.Element {
  const { t } = useI18n()
  const threadId = useCodeSessionStore((state) => state.activeThreadId)
  const lastSeq = useCodeSessionStore((state) => state.lastSeq)
  const toast = useToast()
  const [plan, setPlan] = useState<PlanView | null>(null)
  const [goal, setGoal] = useState('')
  const [newTodo, setNewTodo] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!threadId) {
      setPlan(null)
      return
    }
    const controller = new AbortController()
    void getPlan(threadId, controller.signal)
      .then((next) => {
        setPlan(next)
        setGoal(next.goal ?? '')
        setError(null)
      })
      .catch((reason) => {
        if (!controller.signal.aborted) setError(describe(reason))
      })
    return () => controller.abort()
  }, [threadId, lastSeq])

  if (!threadId) {
    return <EmptyState title={t('workbench.noThreadTitle')} hint={t('workbench.noThreadPlanHint')} />
  }
  if (!plan && !error) return <p className="ark-panel__hint">{t('workbench.plan.loading')}</p>

  const persistTodos = async (todos: TodoEntry[]): Promise<void> => {
    setBusy(true)
    try {
      const next = await updateTodos(threadId, { todos })
      setPlan(next)
      setError(null)
    } catch (reason) {
      const message = describe(reason)
      setError(message)
      toast.push('error', t('workbench.plan.saveTodosFailed'), message)
    } finally {
      setBusy(false)
    }
  }

  const saveGoal = async (): Promise<void> => {
    const text = goal.trim()
    if (!text) return
    setBusy(true)
    try {
      const next = await updateGoal(threadId, text)
      setPlan(next)
      setGoal(next.goal ?? '')
      setError(null)
    } catch (reason) {
      const message = describe(reason)
      setError(message)
      toast.push('error', t('workbench.plan.saveGoalFailed'), message)
    } finally {
      setBusy(false)
    }
  }

  const removeGoal = async (): Promise<void> => {
    setBusy(true)
    try {
      const next = await clearGoal(threadId)
      setPlan(next)
      setGoal('')
      setError(null)
    } catch (reason) {
      const message = describe(reason)
      setError(message)
      toast.push('error', t('workbench.plan.clearGoalFailed'), message)
    } finally {
      setBusy(false)
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
