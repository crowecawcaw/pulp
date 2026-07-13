// A channel's icon, rendered from the shared sprite sheet (`/icons.svg`).
// Two presentations, selected by `bare`:
//   - default ("labeled"): wrapped in a `title`-bearing span with an
//     accessible label on the <svg> itself — used wherever the icon is the
//     only indicator of which channel a mention came from (MentionCard,
//     MentionDetailPage).
//   - bare: icon-only, `aria-hidden`, no wrapper — used where the icon sits
//     next to its own visible text label so the label alone is sufficient
//     for accessibility (FeedPage's channel filter pills).
export function ChannelLogo({ channel, bare = false }: { channel: string; bare?: boolean }) {
  if (bare) {
    return (
      <svg className="ch-logo" aria-hidden="true">
        <use href={`/icons.svg#${channel}-icon`} />
      </svg>
    )
  }
  return (
    <span title={channel}>
      <svg className="ch-logo ch-logo--feed" aria-label={channel} role="img">
        <use href={`/icons.svg#${channel}-icon`} />
      </svg>
    </span>
  )
}
