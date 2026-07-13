import { Link, useParams } from 'react-router-dom'
import { ArrowLeft, ExternalLink, Check, Undo2 } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { useMention, useSetMentionRead, queryKeys } from '@/api/queries'
import { Badge } from '@/components/ui/badge'
import { useToast } from '@/components/ui/useToast'
import { ChannelLogo } from '@/components/ChannelLogo'
import { relativeTime } from '@/lib/time'
import { renderMarkdown } from '@/lib/markdown'

function metaString(meta: Record<string, unknown>, key: string): string | null {
  const v = meta[key]
  return typeof v === 'string' && v.trim() !== '' ? v : null
}

export default function MentionDetailPage() {
  const { id = '' } = useParams<{ id: string }>()
  const { data: mention, isLoading, isError } = useMention(id)
  const setRead = useSetMentionRead()
  const queryClient = useQueryClient()
  const { addToast } = useToast()

  const handleToggleRead = async () => {
    if (!mention) return
    const markRead = mention.read_at == null
    try {
      const updated = await setRead.mutateAsync({ id: mention.id, read: markRead })
      queryClient.setQueryData(queryKeys.mention(mention.id), updated)
      // The feed lists are now stale (read state changed) — let them refetch.
      queryClient.invalidateQueries({ queryKey: ['mentions'] })
    } catch {
      addToast('Failed to update read status', 'error')
    }
  }

  const title = mention ? metaString(mention.platform_meta, 'title') : null
  const storyUrl = mention ? metaString(mention.platform_meta, 'story_url') : null
  const titleHref = storyUrl ?? mention?.content_url

  let body = mention?.content_text ?? ''
  if (title && body.startsWith(title)) body = body.slice(title.length).trim()

  return (
    <div className="page-mid">
      <Link to="/feed" className="back-link">
        <ArrowLeft className="h-3.5 w-3.5" />
        feed
      </Link>

      {isLoading ? (
        <div className="loading-text">loading…</div>
      ) : isError || !mention ? (
        <div className="empty-state">mention not found.</div>
      ) : (
        <div className="card card--padded mention-detail">
          <div className="mention__meta">
            <ChannelLogo channel={mention.channel} />
            <span className="mention-detail__channel">{mention.channel}</span>
            {!mention.read_at && <span className="unread-dot" />}
            <span className="mention__time">{relativeTime(mention.published_at ?? mention.ingested_at)}</span>
          </div>

          {title && (
            <h1 className="mention-detail__title">
              <a href={titleHref} target="_blank" rel="noopener noreferrer">{title}</a>
            </h1>
          )}

          {body.trim() !== '' && (
            <p className="mention__text mention-detail__text">{renderMarkdown(body)}</p>
          )}

          {mention.ai_reason && (
            <p className="mention__ai-reason">AI: {mention.ai_reason}</p>
          )}

          <div className="mention__tags mention-detail__tags">
            {mention.ai_verdict === 'accepted' && (
              <Badge variant="success" title={mention.ai_reason ?? undefined}>AI ✓</Badge>
            )}
            {mention.ai_verdict === 'rejected' && (
              <Badge variant="destructive" title={mention.ai_reason ?? undefined}>AI ✗</Badge>
            )}
            {mention.ai_verdict === 'pending' && <Badge variant="secondary">AI …</Badge>}
          </div>

          {mention.author_name && (
            <p className="mention-detail__author">
              {mention.author_url ? (
                <a href={mention.author_url} target="_blank" rel="noopener noreferrer">
                  {mention.author_name}
                </a>
              ) : (
                mention.author_name
              )}
            </p>
          )}

          <div className="mention-detail__actions">
            <a className="btn btn--outline" href={mention.content_url} target="_blank" rel="noopener noreferrer">
              <ExternalLink className="h-4 w-4" />
              open original
            </a>
            <button className="btn btn--outline" onClick={handleToggleRead} disabled={setRead.isPending}>
              {mention.read_at ? <Undo2 className="h-4 w-4" /> : <Check className="h-4 w-4" />}
              {mention.read_at ? 'mark unread' : 'mark read'}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
