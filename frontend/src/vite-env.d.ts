/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** When "1" or "true", the app runs in demo mode with an in-memory API
   *  (no backend). See `src/demo/`. */
  readonly VITE_DEMO?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
