/// <reference types="vitest/config" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { VitePWA } from 'vite-plugin-pwa'
import path from 'path'

export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      // injectManifest (custom SW) instead of generateSW: a generated Workbox
      // service worker can't carry our `push` / `notificationclick` handlers.
      // src/sw.ts does the precaching AND the Web Push handling.
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      registerType: 'autoUpdate',
      includeAssets: ['favicon.svg', 'apple-touch-icon.png'],
      manifest: {
        name: 'Pulp',
        short_name: 'Pulp',
        description: 'Social listening: monitor conversations across the web',
        start_url: '/',
        scope: '/',
        display: 'standalone',
        theme_color: '#ffffff',
        background_color: '#ffffff',
        icons: [
          { src: '/pwa-192x192.png', sizes: '192x192', type: 'image/png' },
          { src: '/pwa-512x512.png', sizes: '512x512', type: 'image/png' },
          { src: '/pwa-maskable-192x192.png', sizes: '192x192', type: 'image/png', purpose: 'maskable' },
          { src: '/pwa-maskable-512x512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
      injectManifest: {
        // Precache built assets only; never cache /api/* (REST or the SSE
        // stream). The SPA navigation fallback + /api denylist now live in
        // src/sw.ts (NavigationRoute), since injectManifest has no
        // navigateFallback option.
        globPatterns: ['**/*.{js,css,html,svg,png,ico,woff2}'],
      },
    }),
  ],
  build: {
    // Emit the built UI straight into the crate root so `cargo package` can
    // vendor it (rust-embed reads `backend/web-dist`). emptyOutDir is required
    // because the dir is outside the vite root and vite refuses to clean an
    // out-of-root outDir otherwise.
    outDir: '../backend/web-dist',
    emptyOutDir: true,
  },
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') },
  },
  server: {
    host: true,
    proxy: {
      '/api': { target: 'http://localhost:3000', changeOrigin: true },
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    css: true,
    environmentOptions: {
      jsdom: {
        url: 'http://localhost',
      },
    },
  },
})
