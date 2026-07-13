export function relativeTime(epoch: number | null | undefined): string {
  if (!epoch) return 'unknown'
  const diff = Date.now() / 1000 - epoch
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`
  return new Date(epoch * 1000).toLocaleDateString()
}
