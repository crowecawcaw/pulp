import { useState } from 'react'
import { useNavigate, useLocation } from 'react-router-dom'
import { Check, Undo2 } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { ChannelLogo } from '@/components/ChannelLogo'
import { relativeTime } from '@/lib/time'
import { renderMarkdown } from '@/lib/markdown'
import type { Mention } from '@/api/types'

interface MentionCardProps {
  mention: Mention
  monitorPhrase?: string
  onToggleRead?: (mention: Mention) => void
}

function kindLabel(kind: unknown): 'post' | 'comment' | null {
  if (kind === 'comment') return 'comment'
  if (kind === 'story' || kind === 'link') return 'post'
  return null
}

function metaString(meta: Record<string, unknown>, key: string): string | null {
  const v = meta[key]
  return typeof v === 'string' && v.trim() !== '' ? v : null
}

export default function MentionCard({ mention, monitorPhrase, onToggleRead }: MentionCardProps) {
  const [expanded, setExpanded] = useState(false)
  const navigate = useNavigate()
  const location = useLocation()
  const ts = mention.published_at ?? mention.ingested_at
  const isRead = mention.read_at != null

  const kind = kindLabel(mention.platform_meta.kind)
  const storyUrl = metaString(mention.platform_meta, 'story_url')
  const title = metaString(mention.platform_meta, 'title')
  const titleHref = storyUrl ?? mention.content_url

  let body = mention.content_text
  if (title && body.startsWith(title)) body = body.slice(title.length).trim()
  const showBody = body.trim() !== ''

  // The card opens the in-app detail view (full text + open-original + read).
  // The inner source links / buttons stop propagation so they keep their own
  // behavior and don't trigger the card navigation.
  const openDetail = () => navigate(`/mentions/${mention.id}${location.search}`)
  const stop = (e: { stopPropagation: () => void }) => e.stopPropagation()

  return (
    <div
      className={`mention mention--clickable${isRead ? ' mention--read' : ' mention--unread'}`}
      onClick={openDetail}
      tabIndex={0}
      onKeyDown={(e) => { if (e.key === 'Enter') openDetail() }}
    >
      <div className="mention__meta">
        <ChannelLogo channel={mention.channel} />
        {kind && <Badge variant="secondary">{kind}</Badge>}
        {title && (
          <a
            href={titleHref}
            target="_blank"
            rel="noopener noreferrer"
            className="mention__title-link"
            onClick={stop}
          >
            {kind === 'comment' && <span className="mention__on">on </span>}
            {title}
          </a>
        )}
        <a
          href={mention.content_url}
          target="_blank"
          rel="noopener noreferrer"
          className="mention__time"
          onClick={stop}
        >
          {relativeTime(ts)}
        </a>
        {!isRead && <span className="unread-dot" />}
      </div>

      <div className="mention__body">
        {showBody && (
          <p className={`mention__text${expanded ? '' : ' mention__text--clamped'}`}>
            {renderMarkdown(body)}
          </p>
        )}
        {body.length > 200 && (
          <button className="mention__expand" onClick={(e) => { stop(e); setExpanded(!expanded) }}>
            {expanded ? 'show less' : 'show more'}
          </button>
        )}
        {mention.ai_reason && (
          <p className="mention__ai-reason">AI: {mention.ai_reason}</p>
        )}
      </div>

      <div className="mention__footer">
        <div className="mention__tags">
          {mention.ai_verdict === 'accepted' && (
            <Badge variant="success" title={mention.ai_reason ?? undefined}>AI ✓</Badge>
          )}
          {mention.ai_verdict === 'rejected' && (
            <Badge variant="destructive" title={mention.ai_reason ?? undefined}>AI ✗</Badge>
          )}
          {mention.ai_verdict === 'pending' && (
            <Badge variant="secondary">AI …</Badge>
          )}
        </div>

        <div className="mention__actions">
          {monitorPhrase && <span className="mention__monitor">"{monitorPhrase}"</span>}
          {mention.author_name && <span>{mention.author_name}</span>}
          {onToggleRead && (
            <button
              className="mention__read-btn"
              onClick={(e) => { stop(e); onToggleRead(mention) }}
              aria-label={isRead ? 'Mark unread' : 'Mark read'}
            >
              {isRead ? <Undo2 className="h-3.5 w-3.5" /> : <Check className="h-3.5 w-3.5" />}
              {isRead ? 'unread' : 'read'}
            </button>
          )}
        </div>
      </div>
    </div>
  )
}
