import { useEffect, useRef, useState, useMemo, useTransition } from 'react'
import { SlidersHorizontal, X } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { api } from '@/api/client'
import {
  useMonitors, useChannels, useMentions, useSetMentionRead, usePendingCount,
  queryKeys, type MentionsParams,
} from '@/api/queries'
import { isDemoMode } from '@/demo'
import { useWorkspaceStore } from '@/stores/workspace'
import MentionCard from '@/components/MentionCard'
import { ChannelLogo } from '@/components/ChannelLogo'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { useToast } from '@/components/ui/useToast'
import type { Monitor, Mention, MentionPage } from '@/api/types'
import { monitorLabel } from '@/api/types'
import { CHANNELS } from '@/api/channels'

const ALL_CHANNELS: string[] = [...CHANNELS]

const CHANNEL_LABEL: Record<string, string> = {
  hackernews: 'HN', reddit: 'Reddit', github: 'GitHub',
}

const DATE_RANGE_OPTIONS = [
  { label: 'all', days: 0 },
  { label: '30d', days: 30 },
  { label: '7d', days: 7 },
]
const DEFAULT_DATE_RANGE = 0

// Two states only: what the AI filter kept ('relevant' = unfiltered + accepted,
// the feed default) and what it set aside ('filtered out' = rejected). Pending
// items aren't an option here — they surface via the backlog banner instead.
const AI_FILTER_OPTIONS = [
  { value: 'visible',  label: 'relevant' },
  { value: 'rejected', label: 'filtered out' },
]

// Show the "N pending AI filter" banner once the oldest unjudged mention has
// been waiting longer than this — i.e. the worker is genuinely behind, not just
// mid-pass (it runs every ~15s).
const PENDING_STALE_SECS = 15 * 60

const PILL_LIMIT = 4

function FilterPanel({
  monitors,
  enabledChannels,
  selectedMonitor, setSelectedMonitor,
  selectedChannel, setSelectedChannel,
  dateRange, setDateRange,
  showRead, setShowRead,
  aiFilter, setAiFilter,
}: {
  monitors: Monitor[]
  enabledChannels: string[]
  selectedMonitor: string; setSelectedMonitor: (v: string) => void
  selectedChannel: string; setSelectedChannel: (v: string) => void
  dateRange: number; setDateRange: (v: number) => void
  showRead: boolean; setShowRead: (v: boolean) => void
  aiFilter: string; setAiFilter: (v: string) => void
}) {
  const [monitorsExpanded, setMonitorsExpanded] = useState(false)
  const [channelsExpanded, setChannelsExpanded] = useState(false)

  const visibleMonitors = monitorsExpanded ? monitors : monitors.slice(0, PILL_LIMIT)
  const hasMoreMonitors = monitors.length > PILL_LIMIT

  const visibleChannels = channelsExpanded ? enabledChannels : enabledChannels.slice(0, PILL_LIMIT)
  const hasMoreChannels = enabledChannels.length > PILL_LIMIT

  return (
    <div className="filter-form">
      {monitors.length > 0 && (
        <div className="field-group">
          <Label>monitor</Label>
          <div className="f-pill-group">
            <button
              className={`f-pill${selectedMonitor === 'all' ? ' f-pill--on' : ''}`}
              onClick={() => setSelectedMonitor('all')}
            >all</button>
            {visibleMonitors.map((m) => (
              <button
                key={m.id}
                className={`f-pill${selectedMonitor === m.id ? ' f-pill--on' : ''}`}
                onClick={() => setSelectedMonitor(selectedMonitor === m.id ? 'all' : m.id)}
              >{monitorLabel(m)}</button>
            ))}
          </div>
          {hasMoreMonitors && (
            <button className="f-show-more" onClick={() => setMonitorsExpanded((v) => !v)}>
              {monitorsExpanded ? 'show less' : `show ${monitors.length - PILL_LIMIT} more...`}
            </button>
          )}
        </div>
      )}

      <div className="field-group">
        <Label>channel</Label>
        <div className="f-pill-group">
          <button
            className={`f-pill${selectedChannel === 'all' ? ' f-pill--on' : ''}`}
            onClick={() => setSelectedChannel('all')}
          >all</button>
          {visibleChannels.map((c) => (
            <button
              key={c}
              className={`f-pill f-pill--ch${selectedChannel === c ? ' f-pill--on' : ''}`}
              onClick={() => setSelectedChannel(selectedChannel === c ? 'all' : c)}
              title={c}
            >
              <ChannelLogo channel={c} bare />
              {CHANNEL_LABEL[c]}
            </button>
          ))}
        </div>
        {hasMoreChannels && (
          <button className="f-show-more" onClick={() => setChannelsExpanded((v) => !v)}>
            {channelsExpanded ? 'show less' : `show ${enabledChannels.length - PILL_LIMIT} more...`}
          </button>
        )}
      </div>

      <div className="field-group">
        <Label>date range</Label>
        <div className="f-pill-group">
          {DATE_RANGE_OPTIONS.map((opt, idx) => (
            <button
              key={opt.label}
              className={`f-pill${dateRange === idx ? ' f-pill--on' : ''}`}
              onClick={() => setDateRange(idx)}
            >{opt.label}</button>
          ))}
        </div>
      </div>

      <div className="field-group">
        <Label>AI filter</Label>
        <div className="f-pill-group">
          {AI_FILTER_OPTIONS.map((o) => (
            <button
              key={o.value}
              className={`f-pill${aiFilter === o.value ? ' f-pill--on' : ''}`}
              onClick={() => setAiFilter(o.value)}
            >{o.label}</button>
          ))}
        </div>
      </div>

      <div className="field-group">
        <Label>read status</Label>
        <div className="f-pill-group">
          <button
            className={`f-pill${!showRead ? ' f-pill--on' : ''}`}
            onClick={() => setShowRead(false)}
          >unread</button>
          <button
            className={`f-pill${showRead ? ' f-pill--on' : ''}`}
            onClick={() => setShowRead(true)}
          >all</button>
        </div>
      </div>
    </div>
  )
}

export default function FeedPage() {
  const { current } = useWorkspaceStore()
  const { addToast } = useToast()
  const queryClient = useQueryClient()
  const [mobileFiltersOpen, setMobileFiltersOpen] = useState(false)

  const [selectedMonitor, setSelectedMonitor] = useState('all')
  const [selectedChannel, setSelectedChannel] = useState('all')
  const [dateRange, setDateRange] = useState(DEFAULT_DATE_RANGE)
  const [showRead, setShowRead] = useState(false)
  const [aiFilter, setAiFilter] = useState('visible')

  // Pages fetched via "load more", appended after the live first page. Reset
  // whenever the filter params change (the first-page query key changes).
  const [extraPages, setExtraPages] = useState<Mention[]>([])
  const [extraHasMore, setExtraHasMore] = useState<boolean | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)

  const [, startTransition] = useTransition()

  // Store current timestamp for time-dependent calculations (lazy init to avoid calling Date.now in render)
  const [nowSeconds, setNowSeconds] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const interval = setInterval(() => {
      setNowSeconds(Math.floor(Date.now() / 1000))
    }, 5000) // Update every 5 seconds
    return () => clearInterval(interval)
  }, [])

  const { data: monitors = [] } = useMonitors(current?.id)
  const { data: channelConfigs } = useChannels()
  const { data: pending } = usePendingCount(current?.id)

  // Only surface the backlog once it's genuinely stale (oldest item older than
  // the threshold), so a brief between-passes lull doesn't flash a banner.
  const pendingBacklog = useMemo(() => {
    return pending && pending.count > 0 && pending.oldest_ingested_at != null &&
      nowSeconds - pending.oldest_ingested_at > PENDING_STALE_SECS
        ? pending.count
        : 0
  }, [pending, nowSeconds])

  const enabledChannels = useMemo(() => {
    const enabled = (channelConfigs ?? []).filter((c) => c.enabled).map((c) => c.channel)
    return enabled.length > 0 ? enabled : ALL_CHANNELS
  }, [channelConfigs])

  // The first page's params double as the React Query key — filter changes
  // produce a new key and a fresh (cached-then-refetched) first page.
  const params = useMemo<MentionsParams | undefined>(() => {
    if (!current) return undefined
    const p: MentionsParams = { workspace_id: current.id, limit: 20 }
    if (selectedMonitor !== 'all') p.monitor_id = selectedMonitor
    if (selectedChannel !== 'all') p.channel = selectedChannel
    if (!showRead) p.read = false
    if (aiFilter !== 'visible') p.ai = aiFilter
    const range = DATE_RANGE_OPTIONS[dateRange]
    if (range && range.days > 0) {
      p.since = nowSeconds - range.days * 86400
    }
    return p
  }, [current, selectedMonitor, selectedChannel, dateRange, showRead, aiFilter, nowSeconds])

  const { data: firstPage, isLoading, isError } = useMentions(params)

  useEffect(() => {
    if (isError) addToast('Failed to load mentions', 'error')
  }, [isError, addToast])

  // Whenever the filter params change, drop any accumulated "load more" pages.
  useEffect(() => {
    startTransition(() => {
      setExtraPages([])
      setExtraHasMore(null)
    })
  }, [params, startTransition])

  const mentions = useMemo(() => {
    // De-dup in case SSE prepended an item that also appears in a fetched page.
    const firstItems = firstPage?.items ?? []
    const seen = new Set<string>()
    const out: Mention[] = []
    for (const m of [...firstItems, ...extraPages]) {
      if (seen.has(m.id)) continue
      seen.add(m.id)
      out.push(m)
    }
    return out
  }, [firstPage?.items, extraPages])

  const hasMore = extraHasMore ?? firstPage?.has_more ?? false
  const loading = isLoading

  const fetchMore = async () => {
    if (!params) return
    setLoadingMore(true)
    const last = mentions[mentions.length - 1]
    const moreParams: MentionsParams = { ...params }
    // Compound keyset cursor: the effective timestamp (published_at, falling
    // back to ingested_at when null) PLUS the row's id as a tiebreaker. Both
    // halves are required together — a lone `before` has no tiebreak and can
    // skip or duplicate rows that tie on the effective timestamp.
    if (last) {
      moreParams.before = last.published_at ?? last.ingested_at
      moreParams.before_id = last.id
    }
    try {
      const page = await api.mentions.list(moreParams)
      setExtraPages((prev) => [...prev, ...page.items])
      setExtraHasMore(page.has_more)
    } catch {
      addToast('Failed to load mentions', 'error')
    } finally {
      setLoadingMore(false)
    }
  }

  const setMentionRead = useSetMentionRead()
  const sseRef = useRef<EventSource | null>(null)

  // SSE realtime feed: keep the subscription, and on each event push the new
  // mention into the React Query cache for the current first-page key so it
  // shows instantly. The global 5s poll is a backstop.
  useEffect(() => {
    if (!current) return
    if (isDemoMode()) return
    if (!params) return
    if (sseRef.current) sseRef.current.close()
    const sse = new EventSource(`/api/mentions/stream?workspace_id=${current.id}`)
    sse.onmessage = (e) => {
      try {
        const mention = JSON.parse(e.data) as Mention
        // Live mentions arriving on the stream are always feed-visible; don't
        // prepend them while viewing the "filtered out" set.
        if (aiFilter !== 'visible') return
        // A streamed mention must also match every other currently-active
        // filter (channel / monitor / date range) — otherwise a filtered view
        // would transiently show a mention that doesn't belong in it.
        if (params.channel && mention.channel !== params.channel) return
        if (params.monitor_id && mention.monitor_id !== params.monitor_id) return
        if (params.since != null) {
          const ts = mention.published_at ?? mention.ingested_at
          if (ts < params.since) return
        }
        queryClient.setQueryData<MentionPage>(queryKeys.mentions(params), (old) => {
          if (!old) return { items: [mention], has_more: false }
          if (old.items.some((m) => m.id === mention.id)) return old
          return { ...old, items: [mention, ...old.items] }
        })
      } catch {
        // ignore parse errors
      }
    }
    sseRef.current = sse
    return () => sse.close()
  }, [current, aiFilter, params, queryClient])

  const monitorMap = new Map((monitors as Monitor[]).map((k) => [k.id, monitorLabel(k)]))

  const handleToggleRead = async (mention: Mention) => {
    const markRead = mention.read_at == null
    try {
      const updated = await setMentionRead.mutateAsync({ id: mention.id, read: markRead })
      // Update both the live first-page cache and any appended pages.
      queryClient.setQueryData<MentionPage>(queryKeys.mentions(params), (old) => {
        if (!old) return old
        if (markRead && !showRead) return { ...old, items: old.items.filter((m) => m.id !== updated.id) }
        return { ...old, items: old.items.map((m) => (m.id === updated.id ? updated : m)) }
      })
      setExtraPages((prev) => {
        if (markRead && !showRead) return prev.filter((m) => m.id !== updated.id)
        return prev.map((m) => (m.id === updated.id ? updated : m))
      })
    } catch {
      addToast('Failed to update read status', 'error')
    }
  }

  const filterProps = {
    monitors,
    enabledChannels,
    selectedMonitor, setSelectedMonitor,
    selectedChannel, setSelectedChannel,
    dateRange, setDateRange,
    showRead, setShowRead,
    aiFilter, setAiFilter,
  }

  const activeFilterCount = [
    selectedMonitor !== 'all',
    selectedChannel !== 'all',
    dateRange !== DEFAULT_DATE_RANGE,
    showRead,
    aiFilter !== 'visible',
  ].filter(Boolean).length

  const resetFilters = () => {
    setSelectedMonitor('all')
    setSelectedChannel('all')
    setDateRange(DEFAULT_DATE_RANGE)
    setShowRead(false)
    setAiFilter('visible')
  }

  const shownCount = mentions.length
  const unreadCount = mentions.filter((m) => m.read_at == null).length
  // A short, honest summary of what's currently in view — so applying a filter
  // visibly changes the count instead of silently re-listing.
  const resultSummary = loading
    ? null
    : `${shownCount}${hasMore ? '+' : ''} shown${showRead && unreadCount > 0 ? ` · ${unreadCount} unread` : ''}`

  return (
    <div className="feed-layout">
      {/* Desktop filter sidebar */}
      <aside className="hidden lg:block feed-sidebar">
        <div className="feed-sidebar-hd">
          <p className="feed-sidebar-label">filters</p>
          {activeFilterCount > 0 && (
            <button className="filter-clear-btn" onClick={resetFilters}>
              <X className="h-3 w-3" /> clear all
            </button>
          )}
        </div>
        <FilterPanel {...filterProps} />
      </aside>

      {/* Feed column */}
      <div className="feed-main">
        <div className="feed-hd">
          <h1 className="page-title">feed</h1>
          {resultSummary && <span className="feed-count">{resultSummary}</span>}
        </div>

        {/* Mobile filter toggle bar */}
        <div className="lg:hidden filter-bar">
          <button className="filter-toggle-btn" onClick={() => setMobileFiltersOpen((v) => !v)}>
            <SlidersHorizontal className="h-3.5 w-3.5" />
            filters
            {activeFilterCount > 0 && (
              <span className="filter-count-dot">{activeFilterCount}</span>
            )}
          </button>
          {activeFilterCount > 0 && (
            <button className="filter-clear-btn" onClick={resetFilters}>
              <X className="h-3 w-3" /> clear
            </button>
          )}
        </div>

        {/* Mobile filter panel */}
        {mobileFiltersOpen && (
          <div className="lg:hidden filter-panel-mobile">
            <FilterPanel {...filterProps} />
          </div>
        )}

        <div className="feed-content">
          <div className="feed-inner">
            {pendingBacklog > 0 && (
              <div className="feed-pending" role="status">
                {pendingBacklog} {pendingBacklog === 1 ? 'mention' : 'mentions'} pending AI filter…
              </div>
            )}
            {loading ? (
              <div className="empty-state">loading mentions...</div>
            ) : mentions.length === 0 ? (
              showRead ? (
                <div className="empty-state">no mentions found.</div>
              ) : (
                <div className="empty-state">
                  no unread mentions.{' '}
                  <button className="link-inline" onClick={() => setShowRead(true)}>
                    see read mentions
                  </button>
                </div>
              )
            ) : (
              <>
                {mentions.map((m) => (
                  <MentionCard
                    key={m.id}
                    mention={m}
                    monitorPhrase={monitorMap.get(m.monitor_id)}
                    onToggleRead={handleToggleRead}
                  />
                ))}
                {hasMore && (
                  <div className="load-more-row">
                    <Button variant="outline" onClick={fetchMore} disabled={loadingMore}>
                      {loadingMore ? 'loading...' : 'load more'}
                    </Button>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
