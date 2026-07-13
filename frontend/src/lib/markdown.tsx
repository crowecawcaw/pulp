import type { ReactNode } from 'react'

// Inline markdown only — social posts are short and the feed card clamps to a
// few lines, so block constructs (headings, lists, blockquotes) aren't parsed.
// Single-underscore emphasis is intentionally NOT supported: underscores show
// up in usernames and URLs far more often than as italics, so treating them as
// markup would mangle ordinary text. Bold accepts ** or __; italics only *.
const INLINE_SOURCE =
  '(\\*\\*|__)([\\s\\S]+?)\\1' + // 1: bold delimiter, 2: bold inner
  '|\\*([^*\\n]+?)\\*' + //        3: italic inner
  '|`([^`]+?)`' + //               4: inline code
  '|\\[([^\\]]+?)\\]\\((https?:\\/\\/[^\\s)]+)\\)' // 5: link text, 6: url

function renderInline(text: string, keyPrefix: string): ReactNode[] {
  // Fresh regex per call so recursion (e.g. code inside bold) doesn't clobber
  // a shared `lastIndex`.
  const re = new RegExp(INLINE_SOURCE, 'g')
  const nodes: ReactNode[] = []
  let last = 0
  let i = 0
  let m: RegExpExecArray | null
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) nodes.push(text.slice(last, m.index))
    const key = `${keyPrefix}-${i++}`
    if (m[2] !== undefined) {
      nodes.push(<strong key={key}>{renderInline(m[2], key)}</strong>)
    } else if (m[3] !== undefined) {
      nodes.push(<em key={key}>{renderInline(m[3], key)}</em>)
    } else if (m[4] !== undefined) {
      nodes.push(
        <code key={key} className="md-code">
          {m[4]}
        </code>,
      )
    } else if (m[5] !== undefined) {
      nodes.push(
        <a
          key={key}
          href={m[6]}
          target="_blank"
          rel="noopener noreferrer"
          className="md-link"
        >
          {m[5]}
        </a>,
      )
    }
    last = m.index + m[0].length
  }
  if (last < text.length) nodes.push(text.slice(last))
  return nodes
}

/** Render a string of inline markdown to React nodes. */
export function renderMarkdown(text: string): ReactNode {
  return renderInline(text, 'md')
}
