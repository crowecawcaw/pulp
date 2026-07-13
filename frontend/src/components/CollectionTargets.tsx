import { RefreshCw } from 'lucide-react'
import { useChannelTargets } from '@/api/queries'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { relativeTime } from '@/lib/time'
import type { BackfillJob, TargetHealth, TargetStatus } from '@/api/types'

type Props = {
  /** Channel id whose collection targets to show (e.g. `reddit`). */
  channel: string
}

// Health is classified server-side (one classifier shared with the channel
// banner, so the two can't disagree). This is purely the display mapping.
const HEALTH_META: Record<TargetHealth, { label: string; variant: 'success' | 'warning' | 'destructive' | 'secondary'; icon: string }> = {
  healthy:    { label: 'healthy',    variant: 'success',     icon: '✅' },
  throttled:  { label: 'throttled',  variant: 'warning',     icon: '⚠️' },
  failing:    { label: 'failing',    variant: 'destructive', icon: '❌' },
  idle:       { label: 'idle',       variant: 'secondary',   icon: '—' },
}

function openJobsLabel(jobs: BackfillJob[]): string {
  return jobs.length === 0 ? '—' : `${jobs.length}`
}

// One decimal place, but drop a trailing ".0" so "7.5/min" / "8s" read cleanly.
function trim(n: number): string {
  return Number.isFinite(n) ? String(Math.round(n * 10) / 10) : '—'
}

/**
 * The readable health view for a channel's durable collection targets. Fetches
 * `GET /api/channels/{channel}/targets`, derives a status badge per target, and
 * renders a table (desktop) / card list (mobile). Channels on the simple poller
 * return no targets and get a short note instead.
 */
export default function CollectionTargets({ channel }: Props) {
  // 5s background poll comes from the global QueryClient default; the manual
  // refresh button calls `refetch()`.
  const { data, isLoading, isFetching, isError, error, refetch } = useChannelTargets(channel)
  const targets: TargetStatus[] | null = data?.targets ?? null
  const throttle = data?.throttle ?? null

  if (isError) {
    const msg = error instanceof Error ? error.message : 'failed to load targets'
    return <p className="targets__note targets__note--error">{msg}</p>
  }
  if (targets === null) {
    return <p className="targets__note">{isLoading ? 'loading…' : ''}</p>
  }
  if (targets.length === 0) {
    return (
      <p className="targets__note">
        This channel uses the simple poller (no per-target data).
      </p>
    )
  }

  return (
    <div className="targets">
      <div className="targets__bar">
        <span className="targets__count">
          {targets.length} target{targets.length === 1 ? '' : 's'}
          {throttle && (
            <span className="targets__pace">
              pace: ~{trim(throttle.rate_per_min)}/min (1 req / {trim(throttle.interval_secs)}s)
              {throttle.paused && <span className="targets__pace-paused"> paused</span>}
            </span>
          )}
        </span>
        <div className="targets__controls">
          <Button variant="outline" size="sm" onClick={() => refetch()} disabled={isFetching}>
            <RefreshCw className="h-3.5 w-3.5" />
            {isFetching ? 'refreshing…' : 'refresh'}
          </Button>
        </div>
      </div>

      {/* Desktop: table */}
      <div className="hidden sm:block data-table-wrap">
        <table className="data-table">
          <thead>
            <tr>
              <th>status</th>
              <th>kind</th>
              <th>target</th>
              <th>last success</th>
              <th>fails</th>
              <th>watermark</th>
              <th>jobs</th>
            </tr>
          </thead>
          <tbody>
            {targets.map((t) => {
              const meta = HEALTH_META[t.health]
              return (
                <tr key={t.id}>
                  <td>
                    <Badge variant={meta.variant}>{meta.icon} {meta.label}</Badge>
                  </td>
                  <td className="dim">{t.kind}</td>
                  <td>
                    <span className="targets__descriptor" title={t.descriptor}>
                      {t.descriptor}
                    </span>
                  </td>
                  <td className="dim">
                    {t.last_success_at ? relativeTime(t.last_success_at) : 'never'}
                  </td>
                  <td className="dim">{t.consecutive_failures}</td>
                  <td className="dim">
                    {t.confirmed_watermark ? relativeTime(t.confirmed_watermark) : '—'}
                  </td>
                  <td className="dim">{openJobsLabel(t.open_jobs)}</td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>

      {/* Mobile: card list */}
      <div className="sm:hidden item-cards">
        {targets.map((t) => {
          const meta = HEALTH_META[t.health]
          return (
            <div key={t.id} className="card card--padded">
              <div className="targets__card-hd">
                <Badge variant={meta.variant}>{meta.icon} {meta.label}</Badge>
                <span className="dim">{t.kind}</span>
              </div>
              <p className="targets__descriptor" title={t.descriptor}>{t.descriptor}</p>
              <dl className="targets__card-stats">
                <div>
                  <dt>last success</dt>
                  <dd>{t.last_success_at ? relativeTime(t.last_success_at) : 'never'}</dd>
                </div>
                <div>
                  <dt>failures</dt>
                  <dd>{t.consecutive_failures}</dd>
                </div>
                <div>
                  <dt>watermark</dt>
                  <dd>{t.confirmed_watermark ? relativeTime(t.confirmed_watermark) : '—'}</dd>
                </div>
                {t.open_jobs.length > 0 && (
                  <div>
                    <dt>open jobs</dt>
                    <dd>{t.open_jobs.length}</dd>
                  </div>
                )}
              </dl>
              {t.last_error && (
                <p className="targets__error" title={t.last_error}>{t.last_error}</p>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
