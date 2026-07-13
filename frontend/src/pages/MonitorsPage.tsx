import type React from 'react'
import { useEffect, useRef, useState } from 'react'
import { Pencil, Trash2, X } from 'lucide-react'
import { useMonitors, useCreateMonitor, useUpdateMonitor, useDeleteMonitor } from '@/api/queries'
import { useWorkspaceStore } from '@/stores/workspace'
import { useToast } from '@/components/ui/useToast'
import { Button } from '@/components/ui/button'
import { AddButton } from '@/components/ui/add-button'
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
import type { Monitor } from '@/api/types'
import { CHANNELS } from '@/api/channels'

const CHANNEL_LABEL: Record<string, string> = {
  hackernews: 'HN', reddit: 'Reddit', github: 'GitHub',
}

const ALL_CHANNELS = CHANNELS.map((key) => ({ key, label: CHANNEL_LABEL[key] }))

interface MonitorFormState {
  terms: string[]
  channels: string[]
  exact_match: boolean
  case_sensitive: boolean
  exclude_terms: string
  subreddits: string
  only_repos: string
  ai_filter_prompt: string
  channel_settings: Record<string, Record<string, unknown>>
}

const emptyForm = (): MonitorFormState => ({
  terms: [],
  channels: [],
  exact_match: false,
  case_sensitive: false,
  exclude_terms: '',
  subreddits: '',
  only_repos: '',
  ai_filter_prompt: '',
  channel_settings: {},
})

const splitTerms = (s: string) => s.split(',').map((t) => t.trim()).filter(Boolean)

function buildChannelSettings(form: MonitorFormState): Record<string, Record<string, unknown>> {
  const settings: Record<string, Record<string, unknown>> = JSON.parse(
    JSON.stringify(form.channel_settings ?? {}),
  )
  const set = (channel: string, key: string, values: string[]) => {
    if (values.length > 0) {
      settings[channel] = { ...settings[channel], [key]: values }
    } else if (settings[channel]) {
      delete settings[channel][key]
      if (Object.keys(settings[channel]).length === 0) delete settings[channel]
    }
  }
  set('reddit', 'subreddits', splitTerms(form.subreddits).map((s) => s.replace(/^\/?r\//, '')))
  set('github', 'only_repos', splitTerms(form.only_repos))
  return settings
}

// Chip/tags control for a monitor's match-any term list. Type a keyword and
// press Enter or comma to commit it as a chip; click x (or Backspace on an
// empty input) to remove. Each chip is one bare literal term — no OR/quotes.
function TermsInput({
  terms,
  onChange,
}: {
  terms: string[]
  onChange: (terms: string[]) => void
}) {
  const [draft, setDraft] = useState('')

  const commit = (raw: string) => {
    const next = raw.trim()
    if (!next || terms.includes(next)) {
      setDraft('')
      return
    }
    onChange([...terms, next])
    setDraft('')
  }

  const remove = (term: string) => onChange(terms.filter((t) => t !== term))

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      commit(draft)
    } else if (e.key === 'Backspace' && draft === '' && terms.length > 0) {
      remove(terms[terms.length - 1])
    }
  }

  return (
    <div className="term-chips">
      {terms.length > 0 && (
        <div className="row row--wrap">
          {terms.map((t) => (
            <Badge key={t} variant="secondary" className="term-chip">
              {t}
              <button
                type="button"
                className="term-chip__remove"
                aria-label={`remove ${t}`}
                onClick={() => remove(t)}
              >
                <X className="h-3 w-3" />
              </button>
            </Badge>
          ))}
        </div>
      )}
      <Input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={() => commit(draft)}
        placeholder="add a keyword, press Enter"
        aria-label="add keyword"
      />
    </div>
  )
}

// A monitor's channel scoping shown as the full set of channel icons: the ones
// it collects from are lit, the rest greyed out. An empty `channels` list means
// "all channels", so every icon is lit.
function MonitorChannels({ channels }: { channels: string[] }) {
  const all = channels.length === 0
  return (
    <div className="mon-channels">
      {ALL_CHANNELS.map(({ key, label }) => {
        const on = all || channels.includes(key)
        return (
          <svg
            key={key}
            className={`ch-logo mon-ch${on ? ' mon-ch--on' : ' mon-ch--off'}`}
            role="img"
            aria-label={label}
          >
            <title>{label}</title>
            <use href={`/icons.svg#${key}-icon`} />
          </svg>
        )
      })}
    </div>
  )
}

// The monitor's AI filter prompt, shown as its own line (not a pill) so it reads
// at a glance. Clamps to a few lines with a show more/less toggle for the full
// text; the toggle only appears when the text actually overflows the clamp.
function AiFilterLine({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false)
  const [overflows, setOverflows] = useState(false)
  const ref = useRef<HTMLParagraphElement>(null)

  useEffect(() => {
    const el = ref.current
    if (el) setOverflows(el.scrollHeight > el.clientHeight + 1)
  }, [text])

  return (
    <div className="mon-ai">
      <span className="mon-ai__label">AI filter</span>
      <p
        ref={ref}
        className={`mon-ai__text${expanded ? '' : ' mon-ai__text--clamped'}`}
      >
        {text}
      </p>
      {(overflows || expanded) && (
        <button type="button" className="mon-ai__more" onClick={() => setExpanded((v) => !v)}>
          {expanded ? 'show less' : 'show more'}
        </button>
      )}
    </div>
  )
}

export default function MonitorsPage() {
  const { current } = useWorkspaceStore()
  const { addToast } = useToast()
  const { data: monitors = [], isLoading: loading } = useMonitors(current?.id)
  const createMonitor = useCreateMonitor(current?.id)
  const updateMonitor = useUpdateMonitor(current?.id)
  const deleteMonitor = useDeleteMonitor(current?.id)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState<MonitorFormState>(emptyForm())
  const saving = createMonitor.isPending || updateMonitor.isPending

  const openCreate = () => {
    setEditingId(null)
    setForm(emptyForm())
    setDialogOpen(true)
  }

  const openEdit = (mon: Monitor) => {
    setEditingId(mon.id)
    const settings = mon.channel_settings ?? {}
    setForm({
      terms: mon.terms,
      channels: mon.channels,
      exact_match: mon.exact_match,
      case_sensitive: mon.case_sensitive,
      exclude_terms: mon.exclude_terms.join(', '),
      subreddits: ((settings.reddit?.subreddits as string[] | undefined) ?? []).join(', '),
      only_repos: ((settings.github?.only_repos as string[] | undefined) ?? []).join(', '),
      ai_filter_prompt: mon.ai_filter_prompt ?? '',
      channel_settings: settings,
    })
    setDialogOpen(true)
  }

  const handleSave = async () => {
    if (!current || form.terms.length === 0) return
    const data = {
      workspace_id: current.id,
      terms: form.terms,
      channels: form.channels,
      exact_match: form.exact_match,
      case_sensitive: form.case_sensitive,
      exclude_terms: splitTerms(form.exclude_terms),
      channel_settings: buildChannelSettings(form),
      ai_filter_prompt: form.ai_filter_prompt.trim(),
    }
    try {
      if (editingId) {
        await updateMonitor.mutateAsync({ id: editingId, data })
      } else {
        await createMonitor.mutateAsync(data)
      }
      setDialogOpen(false)
      addToast(editingId ? 'Monitor updated' : 'Monitor created', 'success')
    } catch {
      addToast('Failed to save monitor', 'error')
    }
  }

  const handleToggleActive = async (mon: Monitor) => {
    try {
      await updateMonitor.mutateAsync({ id: mon.id, data: { active: !mon.active } })
    } catch {
      addToast('Failed to update monitor', 'error')
    }
  }

  const handleDelete = async (id: string) => {
    if (!confirm('Delete this monitor?')) return
    try {
      await deleteMonitor.mutateAsync(id)
      addToast('Monitor deleted', 'success')
    } catch {
      addToast('Failed to delete monitor', 'error')
    }
  }

  const toggleChannel = (ch: string) => {
    setForm((f) => ({
      ...f,
      channels: f.channels.includes(ch) ? f.channels.filter((c) => c !== ch) : [...f.channels, ch],
    }))
  }

  if (!current) return <div className="page-empty">select a workspace first.</div>

  return (
    <div className="page-wide">
      <div className="page-hd">
        <h1 className="page-title">monitors</h1>
        <AddButton onClick={openCreate}>add monitor</AddButton>
      </div>

      {loading ? (
        <div className="loading-text">loading...</div>
      ) : monitors.length === 0 ? (
        <div className="empty-state">no monitors yet. add one to start monitoring.</div>
      ) : (
        <div className="item-cards">
          {monitors.map((mon) => (
            <div
              key={mon.id}
              className={`card card--padded mon-card${mon.active ? '' : ' mon-card--inactive'}`}
            >
              <div className="mon-card__top">
                <div className="mon-card__terms">
                  {mon.terms.map((t) => (
                    <button
                      key={t}
                      type="button"
                      className="term-pill"
                      onClick={() => openEdit(mon)}
                      title="edit monitor"
                    >
                      {t}
                    </button>
                  ))}
                </div>
                <div className="item-card-actions">
                  <ToggleSwitch checked={mon.active} onChange={() => handleToggleActive(mon)} />
                  <Button variant="ghost" size="sm" aria-label="edit monitor" onClick={() => openEdit(mon)}>
                    <Pencil className="h-3.5 w-3.5" />
                  </Button>
                  <Button variant="ghost" size="sm" className="btn--del" aria-label="delete monitor" onClick={() => handleDelete(mon.id)}>
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
              </div>

              <div className="mon-card__channels-row">
                <MonitorChannels channels={mon.channels} />
                {mon.exact_match && <span className="mon-flag">exact match</span>}
              </div>

              {mon.ai_filter_prompt && <AiFilterLine text={mon.ai_filter_prompt} />}
            </div>
          ))}
        </div>
      )}

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className="max-w-md mx-4 sm:mx-auto">
          <DialogHeader>
            <DialogTitle>{editingId ? 'edit monitor' : 'add monitor'}</DialogTitle>
          </DialogHeader>
          <div className="dialog-form">
            <div className="field-group">
              <Label>keywords (match any of these) *</Label>
              <TermsInput
                terms={form.terms}
                onChange={(terms) => setForm({ ...form, terms })}
              />
              <p className="field-hint">
                a post matches if it contains any one of these. each is a plain keyword or phrase — no OR or quotes.
              </p>
            </div>

            <div className="field-group">
              <Label>channels (empty = all)</Label>
              <div className="form-grid-2">
                {ALL_CHANNELS.map(({ key, label }) => (
                  <label key={key} className="field-check">
                    <input
                      type="checkbox"
                      checked={form.channels.includes(key)}
                      onChange={() => toggleChannel(key)}
                    />
                    {label}
                  </label>
                ))}
              </div>
            </div>

            <div className="form-grid-2">
              <label className="field-check">
                <input
                  type="checkbox"
                  checked={form.exact_match}
                  onChange={(e) => setForm({ ...form, exact_match: e.target.checked })}
                />
                exact match
              </label>
              <label className="field-check">
                <input
                  type="checkbox"
                  checked={form.case_sensitive}
                  onChange={(e) => setForm({ ...form, case_sensitive: e.target.checked })}
                />
                case sensitive
              </label>
            </div>

            <div className="field-group">
              <Label>exclude terms (comma-separated)</Label>
              <Input
                value={form.exclude_terms}
                onChange={(e) => setForm({ ...form, exclude_terms: e.target.value })}
                placeholder="spam, ad, promo"
              />
            </div>

            {(form.channels.length === 0 || form.channels.includes('reddit')) && (
              <div className="field-group">
                <Label>Reddit subreddits (comma-separated; empty = all of Reddit)</Label>
                <Input
                  value={form.subreddits}
                  onChange={(e) => setForm({ ...form, subreddits: e.target.value })}
                  placeholder="accessibility, programming"
                />
              </div>
            )}

            {(form.channels.length === 0 || form.channels.includes('github')) && (
              <div className="field-group">
                <Label>GitHub repos (comma-separated, owner/repo or glob; empty = all)</Label>
                <Input
                  value={form.only_repos}
                  onChange={(e) => setForm({ ...form, only_repos: e.target.value })}
                  placeholder="my-org/*, other/repo"
                />
              </div>
            )}

            <div className="field-group">
              <Label>AI filter prompt (optional)</Label>
              <textarea
                value={form.ai_filter_prompt}
                onChange={(e) => setForm({ ...form, ai_filter_prompt: e.target.value })}
                placeholder="e.g. show me threads where someone could use a desktop automation library built on accessibility APIs"
                rows={3}
                className="field-textarea"
              />
              <p className="field-hint">
                when set, new mentions only reach the feed after the local AI judges them relevant to this prompt.
              </p>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setDialogOpen(false)}>cancel</Button>
            <Button onClick={handleSave} disabled={saving || form.terms.length === 0}>
              {saving ? 'saving...' : 'save'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
