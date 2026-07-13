// capture.mjs — Demo-app screenshot pipeline for Pulp.
//
// Given the built web app running in DEMO MODE (backend-free fixtures), this
// captures a desktop and a mobile shot, frames each (browser-window chrome /
// phone skeleton), and composites both into ONE side-by-side image.
//
// Output: <out>/demo.png
//
// ── Technique: raw capture → CSS-frame composite ─────────────────────────────
// We do it in passes:
//   (1) Navigate DIRECTLY to the demo app (`/feed?demo`) and screenshot the raw
//       viewport at a retina deviceScaleFactor — once for a desktop-sized
//       context, once for a mobile-emulated context.
//   (2) Build ONE wrapper page whose CSS draws both frames (browser window +
//       phone body) side by side and embeds each raw shot as a data-URI <img>.
//       Screenshot just the wrapping `.stage` element.
//
// Why not embed the app in an <iframe> and screenshot in one pass?
//   `?demo` is a RUNTIME toggle: isDemoMode() (frontend/src/demo/index.ts) reads
//   the query param and writes localStorage. A wrapper page built with
//   page.setContent() has an `about:blank` origin, which makes the app a
//   THIRD-PARTY iframe. Chromium's storage partitioning then makes
//   localStorage.setItem() throw, isDemoMode() catches it and returns false, and
//   the app falls back to the (absent) real backend → "Failed to load
//   workspaces". Direct top-level navigation has first-party storage, so demo
//   mode reliably activates. The two-pass composite sidesteps the whole issue
//   and stays crisp: each raw shot is captured at its own high deviceScaleFactor,
//   then embedded at a fixed CSS size in the (lower-dSF) composite pass — more
//   source pixels than the composite needs, never fewer.
//
// Usage:
//   node capture.mjs [--base-url URL] [--out DIR] [--chromium PATH]
//
// Defaults:
//   --base-url  http://localhost:5177   (a static server serving the built app)
//   --out       ./                       (directory next to this script)
//   --chromium  auto — respects PLAYWRIGHT_BROWSERS_PATH; if a pinned
//               /opt/pw-browsers/chromium-*/chrome-linux/chrome exists it is used.

import { chromium } from 'playwright'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { existsSync, readdirSync, readFileSync, unlinkSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))

// ── args ────────────────────────────────────────────────────────────────────
function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`)
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback
}
const BASE_URL = arg('base-url', 'http://localhost:5177').replace(/\/$/, '')
const OUT_DIR = resolve(arg('out', __dirname))
const APP_URL = `${BASE_URL}/feed?demo` // runtime demo toggle — see note above
// Cosmetic address shown in the fake URL bar — not a real navigation target.
const DISPLAY_URL = 'pulp.local/feed'

// Resolve the Chromium binary. Prefer an explicit --chromium, then a pinned
// browser under PLAYWRIGHT_BROWSERS_PATH (this CI box ships one), else let
// Playwright find its own managed download.
function resolveExecutable() {
  const explicit = arg('chromium', null)
  if (explicit && existsSync(explicit)) return explicit
  const root = process.env.PLAYWRIGHT_BROWSERS_PATH
  if (root && existsSync(root)) {
    // Expand `chromium-*/chrome-linux/chrome` by hand — fs.globSync only exists
    // on Node 22+, and CI runs Node 20 (the repo standard).
    const hits = readdirSync(root)
      .filter((name) => name.startsWith('chromium-'))
      .map((name) => `${root}/${name}/chrome-linux/chrome`)
      .filter((p) => existsSync(p))
    if (hits.length) return hits.sort().reverse()[0]
  }
  return undefined // Playwright uses its bundled/managed browser
}

// ── raw capture ──────────────────────────────────────────────────────────────
// Navigate to the demo app top-level (first-party storage → demo mode works),
// wait for the feed to populate, and screenshot the viewport. Returns the PNG
// as a data URI so the wrapper page can embed it with zero external assets.
async function captureRaw(context, outPath) {
  const page = await context.newPage()
  await page.goto(APP_URL, { waitUntil: 'domcontentloaded' })
  // The demo client simulates ~180ms latency; wait for real mention content.
  await page.waitForSelector('.mention', { state: 'visible', timeout: 30_000 })
  // Settle: web fonts + any lazy paint.
  try { await page.evaluate(() => document.fonts?.ready) } catch { /* ignore */ }
  await page.waitForTimeout(600)
  await page.screenshot({ path: outPath }) // viewport-sized (not fullPage)
  await page.close()
  const dataUri = `data:image/png;base64,${readFileSync(outPath).toString('base64')}`
  unlinkSync(outPath) // scratch file only — the composite is the real output
  return dataUri
}

// ── frame markup: browser-window chrome (desktop) ────────────────────────────
// Neutral, theme-agnostic gray browser frame. `imgData` is the raw shot embedded
// at its CSS size (appW×appH).
function windowCss(appW, appH) {
  return `
    .window {
      width: ${appW}px;
      border-radius: 12px;
      overflow: hidden;
      background: #f4f5f7;
      border: 1px solid #d7dae0;
      box-shadow: 0 24px 60px -18px rgba(20,24,31,.45), 0 2px 6px rgba(20,24,31,.12);
    }
    .titlebar {
      display: flex; align-items: center; gap: 12px;
      height: 44px; padding: 0 14px;
      background: linear-gradient(#fbfcfd, #eef0f3);
      border-bottom: 1px solid #dfe2e8;
    }
    .dots { display: flex; gap: 8px; flex: 0 0 auto; }
    .dot { width: 12px; height: 12px; border-radius: 50%; }
    .dot--r { background: #ff5f57; box-shadow: inset 0 0 0 1px rgba(0,0,0,.08); }
    .dot--y { background: #febc2e; box-shadow: inset 0 0 0 1px rgba(0,0,0,.08); }
    .dot--g { background: #28c840; box-shadow: inset 0 0 0 1px rgba(0,0,0,.08); }
    .urlbar {
      flex: 1 1 auto;
      display: flex; align-items: center; gap: 8px;
      height: 28px; padding: 0 12px;
      background: #ffffff;
      border: 1px solid #e0e3e9;
      border-radius: 7px;
      color: #6b7280; font-size: 13px; letter-spacing: .1px;
    }
    .lock { width: 12px; height: 12px; flex: 0 0 auto; opacity: .6; }
    .spacer { flex: 0 0 52px; } /* balances the dots so the URL reads centered */
    .viewport { width: ${appW}px; height: ${appH}px; background: #fff; font-size: 0; }
    .viewport img { width: ${appW}px; height: ${appH}px; display: block; }
  `
}
function windowMarkup(imgData, displayUrl, appW, appH) {
  return `
    <div class="window">
      <div class="titlebar">
        <div class="dots"><span class="dot dot--r"></span><span class="dot dot--y"></span><span class="dot dot--g"></span></div>
        <div class="urlbar">
          <svg class="lock" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>
          <span>${displayUrl}</span>
        </div>
        <div class="spacer"></div>
      </div>
      <div class="viewport" style="width:${appW}px;height:${appH}px"><img src="${imgData}" alt=""></div>
    </div>
  `
}

// ── frame markup: phone skeleton (mobile) ────────────────────────────────────
// Neutral dark phone body: rounded corners, bezel, notch (speaker + camera). The
// raw shot fills the screen area.
function phoneCss(appW, appH) {
  const bezel = 12       // bezel thickness around the screen
  const radius = 52      // outer body corner radius
  const bodyW = appW + bezel * 2
  const bodyH = appH + bezel * 2
  return `
    .phone {
      position: relative;
      width: ${bodyW}px; height: ${bodyH}px;
      background: linear-gradient(160deg, #23262c, #14161a);
      border-radius: ${radius}px;
      padding: ${bezel}px;
      box-shadow:
        0 0 0 2px #3a3e46,
        0 28px 70px -20px rgba(10,12,16,.6),
        inset 0 0 3px rgba(255,255,255,.12);
    }
    .screen {
      position: relative;
      width: ${appW}px; height: ${appH}px;
      border-radius: ${radius - bezel}px;
      overflow: hidden;
      background: #fff; font-size: 0;
    }
    .screen img { width: ${appW}px; height: ${appH}px; display: block; }
    /* Notch: a rounded pill straddling the top of the screen. */
    .notch {
      position: absolute; top: 0; left: 50%; transform: translateX(-50%);
      width: 46%; height: 26px;
      background: #14161a;
      border-bottom-left-radius: 16px; border-bottom-right-radius: 16px;
      z-index: 2;
      display: flex; align-items: center; justify-content: center; gap: 8px;
    }
    .speaker { width: 42px; height: 5px; border-radius: 3px; background: #33363d; }
    .camera { width: 8px; height: 8px; border-radius: 50%; background: #2a2d33; box-shadow: inset 0 0 0 1px #3f434b; }
  `
}
function phoneMarkup(imgData, appW, appH) {
  return `
    <div class="phone">
      <div class="screen">
        <div class="notch"><span class="speaker"></span><span class="camera"></span></div>
        <img src="${imgData}" alt="">
      </div>
    </div>
  `
}

// ── composite: both frames side by side in one shot ──────────────────────────
function composite({ desktopImg, mobileImg, displayUrl, desktopW, desktopH, mobileW, mobileH }) {
  return `<!doctype html><html><head><meta charset="utf-8"><style>
    :root { color-scheme: light; }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    html, body { background: transparent; }
    body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; }
    /* flex: none on both children — otherwise the default flex-shrink lets a
       narrow viewport squeeze these below their fixed width, and since
       .window/.phone clip overflow, that shrink silently crops content
       instead of just overflowing. */
    .stage { display: flex; align-items: center; gap: 56px; padding: 28px; width: fit-content; }
    .stage > * { flex: none; }
    ${windowCss(desktopW, desktopH)}
    ${phoneCss(mobileW, mobileH)}
  </style></head><body>
    <div class="stage">
      ${windowMarkup(desktopImg, displayUrl, desktopW, desktopH)}
      ${phoneMarkup(mobileImg, mobileW, mobileH)}
    </div>
  </body></html>`
}

// Render a wrapper page and screenshot the framed element. The wrapper context
// MUST use a deviceScaleFactor at least as high as any embedded raw shot needs,
// so the embedded images map cleanly to physical pixels (no rescale blur).
async function frame(context, { html, selector, outPath }) {
  const page = await context.newPage()
  await page.setContent(html, { waitUntil: 'load' })
  await page.waitForSelector('img')
  await page.waitForTimeout(100)
  const el = await page.waitForSelector(selector)
  await el.screenshot({ path: outPath })
  await page.close()
  return outPath
}

// ── main ─────────────────────────────────────────────────────────────────────
async function main() {
  const executablePath = resolveExecutable()
  console.log(`app url      : ${APP_URL}`)
  console.log(`out dir      : ${OUT_DIR}`)
  console.log(`chromium     : ${executablePath ?? '(playwright-managed)'}`)

  const browser = await chromium.launch({
    headless: true,
    executablePath,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  })

  const DESKTOP_W = 1440
  const DESKTOP_H = 900
  const MOBILE_W = 390
  const MOBILE_H = 844

  try {
    // ── Desktop: 1440×900 app viewport, retina (dSF 2). ──────────────────────
    const desktopCtx = await browser.newContext({
      viewport: { width: DESKTOP_W, height: DESKTOP_H },
      deviceScaleFactor: 2,
    })
    const desktopImg = await captureRaw(desktopCtx, resolve(OUT_DIR, '.raw-desktop.png'))
    await desktopCtx.close()

    // ── Mobile: 390×844 phone, dSF 3, mobile UA + touch. ─────────────────────
    const mobileCtx = await browser.newContext({
      viewport: { width: MOBILE_W, height: MOBILE_H },
      deviceScaleFactor: 3,
      isMobile: true,
      hasTouch: true,
      userAgent:
        'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1',
    })
    const mobileImg = await captureRaw(mobileCtx, resolve(OUT_DIR, '.raw-mobile.png'))
    await mobileCtx.close()

    // ── Composite: one page, both frames, one screenshot. ────────────────────
    // Viewport is sized generously larger than the stage's content box (see
    // `composite()` for the exact layout) — belt-and-suspenders alongside
    // `.stage > * { flex: none }` so nothing gets flex-shrunk and clipped by
    // .window/.phone's overflow:hidden regardless of default viewport size.
    const stageCtx = await browser.newContext({
      viewport: { width: 2000, height: 1050 },
      deviceScaleFactor: 2,
    })
    const out = resolve(OUT_DIR, 'demo.png')
    await frame(stageCtx, {
      html: composite({
        desktopImg,
        mobileImg,
        displayUrl: DISPLAY_URL,
        desktopW: DESKTOP_W,
        desktopH: DESKTOP_H,
        mobileW: MOBILE_W,
        mobileH: MOBILE_H,
      }),
      selector: '.stage',
      outPath: out,
    })
    await stageCtx.close()
    console.log(`wrote        : ${out}`)
  } finally {
    await browser.close()
  }
}

main().catch((err) => {
  console.error('capture failed:', err)
  process.exit(1)
})
