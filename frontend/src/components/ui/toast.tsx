import * as React from 'react'
import { cn } from '@/lib/utils'
import { ToastContext, type Toast } from './useToast'

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = React.useState<Toast[]>([])

  const addToast = React.useCallback((message: string, type: Toast['type'] = 'info') => {
    const id = Math.random().toString(36).slice(2)
    setToasts((prev) => [...prev, { id, message, type }])
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id))
    }, 4000)
  }, [])

  return (
    <ToastContext.Provider value={{ toasts, addToast }}>
      {children}
      <div className="toast-viewport">
        {toasts.map((t) => (
          <div key={t.id} className={cn('toast', `toast--${t.type}`)}>
            {t.message}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  )
}
