/**
 * Toast surface: one provider, one store, one component. Feature code calls
 * `useToast().push(...)` instead of rendering its own transient banner.
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'

export type ToastTone = 'info' | 'success' | 'error'

export type Toast = {
  id: number
  tone: ToastTone
  message: string
  detail?: string
}

type ToastApi = {
  push: (tone: ToastTone, message: string, detail?: string) => void
  dismiss: (id: number) => void
}

const ToastContext = createContext<ToastApi | null>(null)
const DISMISS_AFTER_MS = 6_000
const MAX_VISIBLE = 4

export function ToastProvider({ children }: { children: ReactNode }): JSX.Element {
  const [toasts, setToasts] = useState<Toast[]>([])
  const nextId = useRef(1)
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>())

  const dismiss = useCallback((id: number) => {
    const timer = timers.current.get(id)
    if (timer) {
      clearTimeout(timer)
      timers.current.delete(id)
    }
    setToasts((rows) => rows.filter((row) => row.id !== id))
  }, [])

  const push = useCallback(
    (tone: ToastTone, message: string, detail?: string) => {
      const id = nextId.current
      nextId.current += 1
      setToasts((rows) => {
        const next = [...rows, { id, tone, message, ...(detail ? { detail } : {}) }]
        return next.slice(-MAX_VISIBLE)
      })
      timers.current.set(
        id,
        setTimeout(() => dismiss(id), DISMISS_AFTER_MS)
      )
    },
    [dismiss]
  )

  useEffect(() => {
    const pending = timers.current
    return () => {
      for (const timer of pending.values()) clearTimeout(timer)
      pending.clear()
    }
  }, [])

  const api = useMemo<ToastApi>(() => ({ push, dismiss }), [dismiss, push])

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div className="ark-toast-stack" role="status" aria-live="polite">
        {toasts.map((toast) => (
          <div key={toast.id} className={`ark-toast ark-toast--${toast.tone}`}>
            <div className="ark-toast__body">
              <span className="ark-toast__message">{toast.message}</span>
              {toast.detail ? <span className="ark-toast__detail">{toast.detail}</span> : null}
            </div>
            <button className="ark-toast__close" type="button" onClick={() => dismiss(toast.id)} aria-label="关闭">
              ×
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  )
}

export function useToast(): ToastApi {
  const api = useContext(ToastContext)
  if (!api) throw new Error('useToast 必须在 ToastProvider 内使用')
  return api
}
