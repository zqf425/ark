import { useEffect, useState } from 'react'
import { ApiError } from '../../../shared/api/client'
import type { PlanView, TodoEntry, TodoStatus } from '../../../shared/contracts'
import { Button, EmptyState, useToast } from '../../../shared/ui'
import { useCodeSessionStore } from '../../code-session/state/store'
import { clearGoal, getPlan, updateGoal, updateTodos } from '../api/workbench-api'

function describe(error: unknown): string {
  if (error instanceof ApiError) return `${error.hint}：${error.message}`
  return error instanceof Error ? error.message : String(error)
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

  if (!threadId) return <EmptyState title="未选择会话" hint="选择会话后可维护其目标和待办。" />
  if (!plan && !error) return <p className="ark-panel__hint">正在加载计划…</p>

  const persistTodos = async (todos: TodoEntry[]): Promise<void> => {
    setBusy(true)
    try {
      const next = await updateTodos(threadId, { todos })
      setPlan(next)
      setError(null)
    } catch (reason) {
      const message = describe(reason)
      setError(message)
      toast.push('error', '保存待办失败', message)
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
      toast.push('error', '保存目标失败', message)
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
      toast.push('error', '清除目标失败', message)
    } finally {
      setBusy(false)
    }
  }

  const todos = plan?.todos ?? []
  return (
    <section className="ark-panel">
      <header className="ark-panel__head">
        <h2>计划</h2>
        <span>{todos.length} 项待办</span>
      </header>
      {error ? <p className="ark-panel__error">{error}</p> : null}

      <label className="ark-panel__label" htmlFor="plan-goal">当前目标</label>
      <textarea id="plan-goal" className="ark-textarea" rows={3} value={goal} disabled={busy} placeholder="描述本会话要达成的结果" onChange={(event) => setGoal(event.target.value)} />
      <div className="ark-panel__actions">
        <Button size="sm" variant="primary" disabled={busy || !goal.trim()} onClick={() => void saveGoal()}>保存目标</Button>
        <Button size="sm" disabled={busy || !plan?.goal} onClick={() => void removeGoal()}>清除</Button>
      </div>

      <div className="ark-plan__list">
        {todos.map((todo) => (
          <div className="ark-plan__todo" key={todo.id}>
            <button type="button" className={`ark-plan__status is-${todo.status}`} disabled={busy} onClick={() => void persistTodos(todos.map((row) => row.id === todo.id ? { ...row, status: nextStatus(row.status) } : row))}>
              {todo.status === 'completed' ? '✓' : todo.status === 'in_progress' ? '•' : '○'}
            </button>
            <span className={todo.status === 'completed' ? 'is-done' : ''}>{todo.title}</span>
            <button type="button" className="ark-linkbtn" disabled={busy} onClick={() => void persistTodos(todos.filter((row) => row.id !== todo.id))}>移除</button>
          </div>
        ))}
      </div>

      <div className="ark-panel__add">
        <input className="ark-field__input" value={newTodo} disabled={busy} placeholder="新增待办" onChange={(event) => setNewTodo(event.target.value)} onKeyDown={(event) => {
          if (event.key === 'Enter' && newTodo.trim()) {
            void persistTodos([...todos, { id: todoId(), title: newTodo.trim(), status: 'pending' }])
            setNewTodo('')
          }
        }} />
        <Button size="sm" disabled={busy || !newTodo.trim()} onClick={() => {
          void persistTodos([...todos, { id: todoId(), title: newTodo.trim(), status: 'pending' }])
          setNewTodo('')
        }}>新增</Button>
      </div>
    </section>
  )
}
