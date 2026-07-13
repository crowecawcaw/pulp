import { useId } from 'react'

// The grapefruit brand mark — a segmented citrus slice (magenta radial flesh, ten
// cream spokes + core). Kept in sync with public/favicon.svg and
// scripts/generate-icons.mjs (the PWA icon generator).
//
// Two forms:
//  - default: cream rind on a diagonal orange→coral→pink gradient tile (matches
//    the installed-app icon).
//  - `bare`: no tile; the rind flips to warm orange so the fruit reads on the
//    light header/sidebar surfaces without a tile behind it.
const SPOKES = [0, 36, 72, 108, 144, 180, 216, 252, 288, 324]

export function GrapefruitLogo({ className, bare = false }: { className?: string; bare?: boolean }) {
  // Scope gradient/filter ids per instance so multiple logos (sidebar + topbar)
  // don't collide on duplicate ids.
  const uid = useId()
  const bg = `${uid}-bg`
  const fl = `${uid}-fl`
  const ds = `${uid}-ds`
  const rind = bare ? '#EE7B33' : '#FCF3E6'
  return (
    <svg
      className={className}
      viewBox="0 0 100 100"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
    >
      <defs>
        {!bare && (
          <linearGradient id={bg} x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#F7A24E" />
            <stop offset="0.5" stopColor="#EE6F3C" />
            <stop offset="1" stopColor="#E8597C" />
          </linearGradient>
        )}
        <radialGradient id={fl} cx="0.4" cy="0.36" r="0.74">
          <stop offset="0" stopColor="#E47096" />
          <stop offset="0.58" stopColor="#D5527A" />
          <stop offset="1" stopColor="#C5446C" />
        </radialGradient>
        <filter id={ds} x="-35%" y="-35%" width="170%" height="170%">
          <feDropShadow dx="0" dy="2" stdDeviation="2.2" floodColor="#5c2a10" floodOpacity="0.3" />
        </filter>
      </defs>
      {/* gradient tile (omitted when bare) */}
      {!bare && <rect width="100" height="100" rx="22" fill={`url(#${bg})`} />}
      {/* rind */}
      <circle cx="50" cy="50" r="40" fill={rind} filter={`url(#${ds})`} />
      {/* flesh + thin outline */}
      <circle cx="50" cy="50" r="31" fill={`url(#${fl})`} />
      <circle cx="50" cy="50" r="31" fill="none" stroke="#B23E62" strokeOpacity="0.22" strokeWidth="1.4" />
      {/* segment spokes */}
      <g stroke="#FCF3E6" strokeWidth="2.6" strokeLinecap="round">
        {SPOKES.map((a) => (
          <line key={a} x1="50" y1="44" x2="50" y2="20" transform={`rotate(${a} 50 50)`} />
        ))}
      </g>
      {/* core */}
      <circle cx="50" cy="50" r="4.6" fill="#FCF3E6" />
      {/* sheen */}
      <ellipse cx="40" cy="36" rx="20" ry="15" fill="#fff" opacity="0.1" />
    </svg>
  );
}
