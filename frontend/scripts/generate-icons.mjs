// One-time PWA icon generator. No dependencies: rasterizes the grapefruit
// icon — a segmented citrus slice (cream rind, magenta radial
// flesh, ten cream spokes + core, soft highlight) on a diagonal
// orange→coral→pink gradient tile — and encodes PNGs by hand with node:zlib.
//
// The design mirrors the GrapefruitLogo SVG (viewBox 0 0 100 100); all
// geometry below is in unit coords (SVG units / 100), centered on (0.5, 0.5).
//
// Usage: node scripts/generate-icons.mjs
import { deflateSync } from 'node:zlib'
import { writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const outDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'public')

// --- minimal PNG encoder (8-bit RGBA) ---------------------------------------

const crcTable = new Int32Array(256).map((_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c
})

function crc32(buf) {
  let c = 0xffffffff
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length)
  out.writeUInt32BE(data.length, 0)
  out.write(type, 4, 'ascii')
  data.copy(out, 8)
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length)
  return out
}

function encodePng(rgba, size) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // color type RGBA
  // scanlines, each prefixed with filter byte 0
  const raw = Buffer.alloc(size * (size * 4 + 1))
  for (let y = 0; y < size; y++) {
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4)
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

// --- grapefruit palette -------------------------------------------------------

const hex = (h) => [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)]

const BG = ['F7A24E', 'EE6F3C', 'E8597C'].map(hex) // diagonal tile gradient stops
const FLESH = ['E47096', 'D5527A', 'C5446C'].map(hex) // radial flesh gradient stops
const CREAM = hex('FCF3E6') // rind, spokes, core
const RING = hex('B23E62') // thin flesh outline
const SHADOW = hex('5C2A10') // soft drop shadow under the rind
const WHITE = [255, 255, 255] // top-left sheen

const hyp = Math.hypot
const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v)
const lerp = (a, b, t) => a + (b - a) * t
const mix = (c1, c2, t) => [lerp(c1[0], c2[0], t), lerp(c1[1], c2[1], t), lerp(c1[2], c2[2], t)]
const over = (base, top, a) => [lerp(base[0], top[0], a), lerp(base[1], top[1], a), lerp(base[2], top[2], a)]
const smoothstep = (e0, e1, x) => {
  const t = clamp((x - e0) / (e1 - e0), 0, 1)
  return t * t * (3 - 2 * t)
}

// 3-stop diagonal background gradient (top-left → bottom-right)
function bgGrad(x, y) {
  const t = clamp((x + y) / 2, 0, 1)
  return t < 0.5 ? mix(BG[0], BG[1], t / 0.5) : mix(BG[1], BG[2], (t - 0.5) / 0.5)
}

// 3-stop radial flesh gradient (offset toward the top-left, like the SVG)
function fleshGrad(d, fr) {
  const t = clamp(d / fr, 0, 1)
  return t < 0.58 ? mix(FLESH[0], FLESH[1], t / 0.58) : mix(FLESH[1], FLESH[2], (t - 0.58) / 0.42)
}

// distance from point (px,py) to segment (ax,ay)-(bx,by)
function sdSeg(px, py, ax, ay, bx, by) {
  const dx = bx - ax, dy = by - ay
  const h = Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
  return hyp(px - ax - dx * h, py - ay - dy * h)
}

// Composite the grapefruit over the gradient tile at unit coords (x,y).
// `s` scales the whole fruit about the center (s=1 → rind radius 0.40), so
// maskable/apple variants can shrink it into their safe zone.
function grapefruit(x, y, s) {
  const dx = x - 0.5, dy = y - 0.5
  const d = hyp(dx, dy)

  let col = bgGrad(x, y)

  // soft drop shadow: offset down by 0.02·s, fades out past the rind edge
  const dsh = hyp(dx, dy - 0.02 * s)
  const shA = 0.3 * (1 - smoothstep(0.4 * s - 0.006, 0.43 * s, dsh))
  if (shA > 0.001) col = over(col, SHADOW, shA)

  const rindR = 0.4 * s
  if (d > rindR) return col

  // rind
  col = CREAM

  const fleshR = 0.31 * s
  if (d <= fleshR) {
    // flesh radial gradient (center offset up-left, matching the SVG)
    const fd = hyp(x - (0.5 - 0.062 * s), y - (0.5 - 0.0868 * s))
    col = fleshGrad(fd, 0.4588 * s)
  }

  // thin flesh outline ring
  if (Math.abs(d - fleshR) <= 0.007 * s) col = over(col, RING, 0.22)

  // ten cream spokes every 36° — rotate the point into the frame of the
  // angularly-nearest spoke (which points "up"), then test that one segment.
  const STEP = Math.PI / 5 // 36°
  const phi = Math.atan2(dx, -dy) // spoke-angle: 0 = up, +ve clockwise
  const beta = phi - Math.round(phi / STEP) * STEP
  const rdx = d * Math.sin(beta)
  const rdy = -d * Math.cos(beta)
  if (sdSeg(rdx, rdy, 0, -0.06 * s, 0, -0.3 * s) <= 0.013 * s) col = CREAM

  // core dot
  if (d <= 0.046 * s) col = CREAM

  // top-left sheen
  const ex = (x - (0.5 - 0.1 * s)) / (0.2 * s)
  const ey = (y - (0.5 - 0.14 * s)) / (0.15 * s)
  if (ex * ex + ey * ey <= 1) col = over(col, WHITE, 0.1)

  return col
}

// rounded-rect signed distance (unit coords), corner radius rr; null rr = square
function sdRoundRect(x, y, rr) {
  const qx = Math.abs(x - 0.5) - (0.5 - rr)
  const qy = Math.abs(y - 0.5) - (0.5 - rr)
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - rr
}

// --- icon rendering ----------------------------------------------------------
//
// scale — fruit scale about center (1 → rind radius 0.40 of the tile)
// rr     — tile corner radius; null means a full opaque square (full bleed)
function render(size, { scale, rr }) {
  const ss = 4 // supersampling per axis
  const rgba = Buffer.alloc(size * size * 4)
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let r = 0, g = 0, b = 0, a = 0
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const x = (px + (sx + 0.5) / ss) / size
          const y = (py + (sy + 0.5) / ss) / size
          const inTile = rr == null ? true : sdRoundRect(x, y, rr) <= 0
          if (!inTile) continue
          const [cr, cg, cb] = grapefruit(x, y, scale)
          r += cr; g += cg; b += cb; a += 255
        }
      }
      const n = ss * ss
      const i = (py * size + px) * 4
      const cov = a / n
      rgba[i] = cov ? Math.round(r / (a / 255)) : 0
      rgba[i + 1] = cov ? Math.round(g / (a / 255)) : 0
      rgba[i + 2] = cov ? Math.round(b / (a / 255)) : 0
      rgba[i + 3] = Math.round(cov)
    }
  }
  return encodePng(rgba, size)
}

mkdirSync(outDir, { recursive: true })
const icons = [
  // Standard (any-purpose) icons: fruit on a rounded gradient tile.
  ['pwa-192x192.png', 192, { scale: 1.0, rr: 0.22 }],
  ['pwa-512x512.png', 512, { scale: 1.0, rr: 0.22 }],
  // Maskable icons: gradient fills the whole square; fruit shrinks into the
  // maskable safe zone (a circle of radius 0.4) so a circular mask won't clip.
  ['pwa-maskable-192x192.png', 192, { scale: 0.85, rr: null }],
  ['pwa-maskable-512x512.png', 512, { scale: 0.85, rr: null }],
  // iOS rounds corners itself; must be an opaque full square.
  ['apple-touch-icon.png', 180, { scale: 0.95, rr: null }],
]
for (const [name, size, opts] of icons) {
  writeFileSync(join(outDir, name), render(size, opts))
  console.log(`wrote public/${name}`)
}
