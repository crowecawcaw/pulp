import { Link, useParams } from 'react-router-dom'
import { ArrowLeft } from 'lucide-react'
import { useChannel, useChannelTargets } from '@/api/queries'
import { Badge } from '@/components/ui/badge'
import LogViewer from '@/components/LogViewer'
import CollectionTargets from '@/components/CollectionTargets'
import { relativeTime } from '@/lib/time'
import type { Channel } from '@/api/channels'

// Display labels for the known channels (`Record<Channel, ...>` means adding a
// channel to `CHANNELS` without a label here is a compile error). Falls back
// to the raw id for anything not listed, so the page works even before a
// channel has been configured.
const CHANNEL_LABELS: Record<Channel, string> = {
  hackernews: 'Hacker News',
  reddit: 'Reddit',
  github: 'GitHub',
}

export default function ChannelDetailPage() {
  const { channel = '' } = useParams<{ channel: string }>()
  // A channel that has never been configured 404s; that's fine — show the
  // header from defaults and still render the log viewer. `useChannel` disables
  // retries for exactly this case, so the error simply means "no config".
  const { data, isLoading, isError } = useChannel(channel)
  const config = isError ? null : (data ?? null)
  const loading = isLoading

  // The degraded banner is sourced from the SAME live targets query the
  // per-target table below uses (shared cache, 5s poll), so the banner count and
  // the table can never disagree. Simple-poller channels (no targets) fall back
  // to the stored channel-level error_message.
  const { data: targetsData } = useChannelTargets(channel)
  const bannerMessage = targetsData?.summary?.message ?? config?.error_message ?? null

  const label = CHANNEL_LABELS[channel as Channel] ?? channel

  return (
    <div className="page-wide">
      <Link to="/channels" className="back-link">
        <ArrowLeft className="h-3.5 w-3.5" />
        channels
      </Link>

      <div className="row row--between channel-detail__header">
        <h1 className="page-title">{label}</h1>
        {config && (
          <Badge variant={config.enabled ? 'success' : 'secondary'}>
            {config.enabled ? 'enabled' : 'disabled'}
          </Badge>
        )}
      </div>

      {/* Channel-level degraded summary — surfaced prominently when present.
          Live from the targets summary (consistent with the table below). */}
      {bannerMessage && (
        <div className="channel-banner" role="status">
          <span className="channel-banner__label">degraded</span>
          <span className="channel-banner__text">{bannerMessage}</span>
        </div>
      )}

      <div className="card card--padded channel-detail__summary">
        {loading ? (
          <p className="loading-text">loading…</p>
        ) : (
          <dl className="channel-summary">
            <div>
              <dt>last polled</dt>
              <dd>{config?.last_polled_at ? relativeTime(config.last_polled_at) : 'never'}</dd>
            </div>
            <div>
              <dt>poll interval</dt>
              <dd>{config?.poll_interval ? `${config.poll_interval}s` : '—'}</dd>
            </div>
          </dl>
        )}
      </div>

      {/* Primary content: the readable per-target health view. */}
      <div className="section-sep">
        <p className="section-sep__title">Collection targets</p>
        <p className="section-sep__desc">
          Per-target collection health — watermark, failures and backoff for each
          upstream this channel polls.
        </p>
        <CollectionTargets channel={channel} />
      </div>

      {/* Secondary: the raw collector log. */}
      <div className="section-sep">
        <p className="section-sep__title">Recent logs</p>
        <p className="section-sep__desc">
          The most recent server log lines for this channel's collector.
        </p>
        <LogViewer service={channel} />
      </div>
    </div>
  )
}
