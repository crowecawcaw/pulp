# Pulp Frontend

React 19 + TypeScript + Vite + Tailwind CSS + shadcn/ui frontend for **Pulp**, a self-hostable social-listening tool. Monitors Reddit, GitHub, Twitter/X and other channels for keyword mentions, applies optional AI-powered relevance filtering, and delivers alerts via webhooks or Web Push.

## Quick start

```bash
npm install
npm run dev          # http://localhost:5173
npm run build        # production build
npm test             # run tests
```

API types in `src/api/types.gen.ts` are **generated from the backend's OpenAPI spec** and must not be hand-edited. Regenerate after the backend schema changes:

```bash
npm run gen:api      # from a running backend (http://localhost:3000)
npm run gen:api:file # from the checked-in ../backend/openapi.json (offline)
```

See the root README and AGENTS.md for full project documentation.
