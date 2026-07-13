import { useEffect, useRef, useMemo } from 'react'
import { RefreshCw } from 'lucide-react'
import { useLogs } from '@/api/queries'
import { Button } from '@/components/ui/button'

type Props = {
  /** Service id whose logs to show: a channel name (`reddit`, `github`, …) or
   *  a future service such as `ai_filter` / `llm`. Generic by design. */
  service: string
  /** Max lines to request (default 200, backend caps at 2000). */
  limit?: number
}

/**
 * Reusable scrollable log viewer. Reads `GET /api/logs/{service}` via React
 * Query and renders the tail in a monospace box. The 5s background poll comes
 * from the global QueryClient default; the manual refresh button calls
 * `refetch()`.
 */
export default function LogViewer({ service, limit = 200 }: Props) {
  const { data, isFetching, isError, error, refetch } = useLogs(service, limit)
  const lines = useMemo(() => data?.lines ?? [], [data?.lines])
  const exists = data?.exists ?? true
  const preRef = useRef<HTMLPreElement>(null)
  // Track whether the user is scrolled to the bottom so auto-refresh only
  // auto-scrolls when they're following the tail.
  const atBottomRef = useRef(true)

  // Keep the view pinned to the newest line when the user is at the bottom.
  useEffect(() => {
    const el = preRef.current
    if (el && atBottomRef.current) {
      el.scrollTop = el.scrollHeight
    }
  }, [lines])

  const onScroll = () => {
    const el = preRef.current
    if (!el) return
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
  }

  const errMsg = isError ? (error instanceof Error ? error.message : 'failed to load logs') : null

  return (
    <div className="log-viewer">
      <div className="log-viewer__bar">
        <div className="log-viewer__meta">
          <span>{lines.length} line{lines.length === 1 ? '' : 's'}</span>
          {!exists && <span>· no log file yet</span>}
        </div>
        <div className="log-viewer__controls">
          <Button variant="outline" size="sm" onClick={() => refetch()} disabled={isFetching}>
            <RefreshCw className="h-3.5 w-3.5" />
            {isFetching ? 'refreshing…' : 'refresh'}
          </Button>
        </div>
      </div>

      <pre className="log-viewer__pre" ref={preRef} onScroll={onScroll}>
        {errMsg ? (
          <span className="log-viewer__error">{errMsg}</span>
        ) : lines.length === 0 ? (
          <span className="log-viewer__empty">
            {isFetching ? 'loading…' : 'no log output for this service yet.'}
          </span>
        ) : (
          lines.map((line, i) => (
            <span className="log-viewer__line" key={i}>{line}</span>
          ))
        )}
      </pre>
    </div>
  )
}
