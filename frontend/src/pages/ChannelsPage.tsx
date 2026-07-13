import { useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Settings, ScrollText } from 'lucide-react'
import { useChannels, useUpdateChannel, useChannelCleanup, useChannelBackfill } from '@/api/queries'
import { useToast } from '@/components/ui/useToast'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { ToggleSwitch } from '@/components/ui/toggle-switch'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { relativeTime } from '@/lib/time'
import type { ChannelConfig, CleanupPreview, CleanupResult } from '@/api/types'
import { CHANNELS, type Channel } from '@/api/channels'

type FieldDef = {
  key: string;
  label: string;
  type?: 'password' | 'textarea' | 'patterns' | 'select';
  placeholder?: string;
  options?: Array<{ value: string; label: string }>;
}

// Keyed by channel name; `ch.channel` from the API is a plain `string`, so this
// stays a `Record<string, ...>` (an index type of `Channel` would reject that
// lookup) — `ALL_CHANNELS` below is what keeps this in sync with `CHANNELS`.
const CHANNEL_DISPLAY: Record<string, { label: string; fields: Array<FieldDef> }> = {
  hackernews:   { label: 'Hacker News', fields: [] },
  reddit:       { label: 'Reddit', fields: [
    { key: 'user_agent',          label: 'User Agent', placeholder: 'nimbus:v0.1 (by /u/nimbuslabs)' },
    { key: 'subreddits',          label: 'Subreddits (one per line, leave blank to search all of Reddit)', type: 'patterns', placeholder: 'selfhosted\ndataengineering' },
    { key: 'exclude_subreddits',  label: 'Exclude subreddits (one per line)', type: 'patterns', placeholder: 'deals' },
    { key: 'exclude_authors',     label: 'Exclude authors (one per line)', type: 'patterns', placeholder: 'spammer' },
  ]},
  github:       { label: 'GitHub', fields: [
    { key: 'token',          label: 'Personal Access Token', type: 'password' },
    { key: 'ignore_repos',   label: 'Ignore repos (glob patterns, one per line)', type: 'patterns', placeholder: 'nimbus-labs/*\nmyorg/specific-repo' },
    { key: 'ignore_orgs',    label: 'Ignore orgs (one per line)', type: 'patterns', placeholder: 'myorg' },
    { key: 'ignore_authors', label: 'Ignore authors (one per line)', type: 'patterns', placeholder: 'my-github-username' },
    { key: 'only_repos',     label: 'Only watch these repos (glob patterns, leave blank for all)', type: 'patterns', placeholder: 'partner-org/*' },
    { key: 'state_filter',   label: 'Issue state filter', type: 'select', options: [
      { value: 'open',   label: 'Open only (recommended)' },
      { value: 'all',    label: 'All states' },
      { value: 'closed', label: 'Closed only' },
    ]},
  ]},
}

const ALL_CHANNELS: readonly Channel[] = CHANNELS

export default function ChannelsPage() {
  const { addToast } = useToast()
  const navigate = useNavigate()
  const { data: channelList, isLoading: loading } = useChannels()
  const updateChannel = useUpdateChannel()
  const cleanup = useChannelCleanup()
  const backfill = useChannelBackfill()
  const [configChannel, setConfigChannel] = useState<ChannelConfig | null>(null)
  const [credFields, setCredFields] = useState<Record<string, string>>({})
  const saving = updateChannel.isPending
  const [backfillDays, setBackfillDays] = useState<number>(30)
  const backfilling = backfill.isPending
  const [cleanupPreview, setCleanupPreview] = useState<CleanupPreview | null>(null)
  const cleaningUp = cleanup.isPending && !cleanup.variables?.dryRun
  const checkingCleanup = cleanup.isPending && cleanup.variables?.dryRun === true

  // Always render the full ALL_CHANNELS set, backfilling configured rows with
  // defaults for any channel that has no saved config yet.
  const channels = useMemo<ChannelConfig[]>(() => {
    const map = new Map((channelList ?? []).map((c) => [c.channel, c]))
    return ALL_CHANNELS.map((ch) => map.get(ch) ?? {
      channel: ch,
      enabled: false,
      credentials: {},
      poll_interval: 300,
      last_polled_at: null,
      error_message: null,
      updated_at: 0,
    })
  }, [channelList])

  const handleToggle = async (ch: ChannelConfig) => {
    try {
      await updateChannel.mutateAsync({ channel: ch.channel, data: { enabled: !ch.enabled } })
    } catch {
      addToast('Failed to update channel', 'error')
    }
  }

  const openConfig = (ch: ChannelConfig) => {
    setConfigChannel(ch)
    const fields: Record<string, string> = {}
    const def = CHANNEL_DISPLAY[ch.channel]
    if (def) {
      def.fields.forEach((f) => {
        const raw = ch.credentials[f.key]
        if (f.type === 'patterns' && Array.isArray(raw)) {
          fields[f.key] = (raw as string[]).join('\n')
        } else if (f.type === 'select') {
          fields[f.key] = (raw as string) ?? f.options?.[0]?.value ?? ''
        } else {
          fields[f.key] = (raw as string) ?? ''
        }
      })
    }
    setCredFields(fields)
  }

  const handleSaveConfig = async () => {
    if (!configChannel) return
    try {
      const credentials: Record<string, unknown> = {}
      Object.entries(credFields).forEach(([k, v]) => {
        const fieldDef = CHANNEL_DISPLAY[configChannel.channel]?.fields.find(f => f.key === k)
        if (fieldDef?.type === 'patterns' || k === 'feed_urls') {
          credentials[k] = v.split('\n').map((u) => u.trim()).filter(Boolean)
        } else {
          credentials[k] = v
        }
      })
      await updateChannel.mutateAsync({ channel: configChannel.channel, data: { credentials } })
      setConfigChannel(null)
      addToast('Channel updated', 'success')
    } catch {
      addToast('Failed to save channel config', 'error')
    }
  }

  const handleCheckCleanup = async () => {
    if (!configChannel) return
    setCleanupPreview(null)
    try {
      const result = await cleanup.mutateAsync({ channel: configChannel.channel, dryRun: true }) as CleanupPreview
      setCleanupPreview(result)
    } catch {
      addToast('Failed to check for noise', 'error')
    }
  }

  const handleCleanup = async () => {
    if (!configChannel) return
    try {
      const result = await cleanup.mutateAsync({ channel: configChannel.channel, dryRun: false }) as CleanupResult
      setCleanupPreview(null)
      addToast(`Deleted ${result.deleted} noisy mention${result.deleted === 1 ? '' : 's'}`, 'success')
    } catch {
      addToast('Failed to clean up mentions', 'error')
    }
  }

  const handleBackfill = async () => {
    if (!configChannel) return
    try {
      await backfill.mutateAsync({ channel: configChannel.channel, days: backfillDays })
      addToast(`Backfill complete for the last ${backfillDays} days`, 'success')
    } catch {
      addToast('Backfill failed', 'error')
    }
  }

  return (
    <div className="page-wide">
      <div className="page-hd">
        <h1 className="page-title">channels</h1>
      </div>

      {loading ? (
        <div className="loading-text">loading...</div>
      ) : (
        <>
          {/* Mobile: card list */}
          <div className="sm:hidden item-cards">
            {channels.map((ch) => {
              const def = CHANNEL_DISPLAY[ch.channel]
              return (
                <div key={ch.channel} className="card card--padded">
                  <div className="item-card-row">
                    <div className="item-card-info">
                      <button
                        className="channel-name-link item-card-name"
                        onClick={() => navigate(`/channels/${ch.channel}`)}
                      >
                        {def?.label ?? ch.channel}
                      </button>
                      <p className="item-card-sub">
                        {ch.last_polled_at ? `polled ${relativeTime(ch.last_polled_at)}` : 'never polled'}
                      </p>
                      {ch.error_message && (
                        <p className="item-card-sub item-card-sub--error">{ch.error_message}</p>
                      )}
                    </div>
                    <div className="item-card-actions">
                      <Badge variant={ch.enabled ? 'success' : 'secondary'}>
                        {ch.enabled ? 'on' : 'off'}
                      </Badge>
                      <ToggleSwitch checked={ch.enabled} onChange={() => handleToggle(ch)} />
                      <Button variant="ghost" size="sm" aria-label="View logs" onClick={() => navigate(`/channels/${ch.channel}`)}>
                        <ScrollText className="h-3.5 w-3.5" />
                      </Button>
                      {def && def.fields.length > 0 && (
                        <Button variant="ghost" size="sm" onClick={() => openConfig(ch)}>
                          <Settings className="h-3.5 w-3.5" />
                        </Button>
                      )}
                    </div>
                  </div>
                </div>
              )
            })}
          </div>

          {/* Desktop: table */}
          <div className="hidden sm:block data-table-wrap">
            <table className="data-table">
              <thead>
                <tr>
                  <th>channel</th>
                  <th>status</th>
                  <th>last polled</th>
                  <th>error</th>
                  <th>actions</th>
                </tr>
              </thead>
              <tbody>
                {channels.map((ch) => {
                  const def = CHANNEL_DISPLAY[ch.channel]
                  return (
                    <tr key={ch.channel}>
                      <td>
                        <button className="channel-name-link" onClick={() => navigate(`/channels/${ch.channel}`)}>
                          {def?.label ?? ch.channel}
                        </button>
                      </td>
                      <td>
                        <Badge variant={ch.enabled ? 'success' : 'secondary'}>
                          {ch.enabled ? 'enabled' : 'disabled'}
                        </Badge>
                      </td>
                      <td className="dim">
                        {ch.last_polled_at ? relativeTime(ch.last_polled_at) : 'never'}
                      </td>
                      <td>
                        {ch.error_message && (
                          <span className="channels-table__error">
                            {ch.error_message}
                          </span>
                        )}
                      </td>
                      <td>
                        <div className="row row--end">
                          <ToggleSwitch checked={ch.enabled} onChange={() => handleToggle(ch)} />
                          <Button variant="ghost" size="sm" aria-label="View logs" onClick={() => navigate(`/channels/${ch.channel}`)}>
                            <ScrollText className="h-3.5 w-3.5" />
                          </Button>
                          {def && def.fields.length > 0 && (
                            <Button variant="ghost" size="sm" onClick={() => openConfig(ch)}>
                              <Settings className="h-3.5 w-3.5" />
                            </Button>
                          )}
                        </div>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        </>
      )}

      <Dialog open={!!configChannel} onOpenChange={(open) => { if (!open) { setConfigChannel(null); setCleanupPreview(null) } }}>
        <DialogContent className="mx-4 sm:mx-auto">
          <DialogHeader>
            <DialogTitle className="lowercase">
              configure {configChannel ? (CHANNEL_DISPLAY[configChannel.channel]?.label ?? configChannel.channel) : ''}
            </DialogTitle>
          </DialogHeader>
          <div className="dialog-form">
            {configChannel && CHANNEL_DISPLAY[configChannel.channel]?.fields.map((field) => (
              <div key={field.key} className="field-group">
                <Label>{field.label}</Label>
                {field.type === 'textarea' || field.type === 'patterns' ? (
                  <textarea
                    className="field-textarea"
                    value={credFields[field.key] ?? ''}
                    onChange={(e) => setCredFields({ ...credFields, [field.key]: e.target.value })}
                    placeholder={field.type === 'textarea' ? 'One URL per line' : field.placeholder}
                  />
                ) : field.type === 'select' ? (
                  <select
                    className="field-select"
                    value={credFields[field.key] ?? field.options?.[0]?.value ?? ''}
                    onChange={(e) => setCredFields({ ...credFields, [field.key]: e.target.value })}
                  >
                    {field.options?.map((opt) => (
                      <option key={opt.value} value={opt.value}>{opt.label}</option>
                    ))}
                  </select>
                ) : (
                  <Input
                    type={field.type ?? 'text'}
                    value={credFields[field.key] ?? ''}
                    onChange={(e) => setCredFields({ ...credFields, [field.key]: e.target.value })}
                    placeholder={field.placeholder}
                  />
                )}
              </div>
            ))}

            <div className="section-sep">
              <p className="section-sep__title">Backfill history</p>
              <p className="section-sep__desc">Fetch mentions from the past N days for this channel.</p>
              <div className="backfill-row">
                <input
                  type="number"
                  min={1}
                  max={365}
                  value={backfillDays}
                  onChange={(e) => setBackfillDays(Number(e.target.value))}
                  className="field-number"
                />
                <span className="dim">days</span>
                <Button variant="outline" size="sm" onClick={handleBackfill} disabled={backfilling}>
                  {backfilling ? 'running...' : 'start backfill'}
                </Button>
              </div>
            </div>

            {configChannel?.channel === 'github' && (
              <div className="section-sep">
                <p className="section-sep__title">Remove old noise</p>
                <p className="section-sep__desc">
                  Preview and remove existing mentions that match your current ignore settings.
                </p>
                {cleanupPreview === null ? (
                  <Button variant="outline" size="sm" onClick={handleCheckCleanup} disabled={checkingCleanup}>
                    {checkingCleanup ? 'checking...' : 'check for noise'}
                  </Button>
                ) : (
                  <div className="stack--sm">
                    <p className="cleanup-summary">
                      {cleanupPreview.count === 0
                        ? 'no mentions match the current ignore settings.'
                        : `${cleanupPreview.count} mention${cleanupPreview.count === 1 ? '' : 's'} would be removed.`}
                    </p>
                    {cleanupPreview.sample.length > 0 && (
                      <ul className="cleanup-list">
                        {cleanupPreview.sample.map((item) => (
                          <li key={item.id}>
                            <strong>{item.repo ?? 'unknown'}</strong>
                            {item.author && <span> · {item.author}</span>}
                            {' — '}{item.title}
                          </li>
                        ))}
                      </ul>
                    )}
                    {cleanupPreview.count > 0 && (
                      <div className="row">
                        <Button variant="destructive" size="sm" onClick={handleCleanup} disabled={cleaningUp}>
                          {cleaningUp ? 'deleting...' : `delete ${cleanupPreview.count} mention${cleanupPreview.count === 1 ? '' : 's'}`}
                        </Button>
                        <Button variant="outline" size="sm" onClick={() => setCleanupPreview(null)}>cancel</Button>
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfigChannel(null)}>cancel</Button>
            <Button onClick={handleSaveConfig} disabled={saving}>
              {saving ? 'saving...' : 'save'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
