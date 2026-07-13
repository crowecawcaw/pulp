---
name: fixtures
description: Canonical fictional scenarios (Nimbus, Fern) that all fabricated Pulp data must draw from. Use whenever adding or editing backend seed data (backend/src/cli/seed.rs), frontend demo fixtures (frontend/src/demo/fixtures.ts), unit/integration test fixtures, mock-server responses, or any example data in docs — anywhere you'd otherwise invent a company, product, username, or URL for Pulp.
---

# Test & demo data — canonical scenarios

**All fabricated data — backend `pulp seed`, the frontend demo fixtures
(`frontend/src/demo/fixtures.ts`), unit/integration test fixtures, mock-server
responses, and every example in the docs — must draw from the two fictional
scenarios below and nothing else.** Keep the cast small and consistent so the
demo reads as one coherent world and so a reader can't mistake a fixture for a
real endorsement.

Hard rules:

- **No personal data.** No real names, emails, usernames, handles, Tailscale
  hostnames, API tokens, or infrastructure. Author handles are invented and
  obviously fabricated (`ops_marlowe`, `u/dataeng_dan`, `kade_rios`, …).
- **No real products, services, companies, or people.** Everything named is one
  of the fictional entities below. Do **not** reference real tools as
  competitors/alternatives or real SaaS in examples (no Slack, Datadog, Postgres,
  Hugo, AWS-by-name, etc.). "the usual alternatives" or a fictional name instead.
- **Fictional URLs use the reserved `.example` TLD** (`nimbus.example`,
  `hooks.example.com`, `push.example.com`). Never a real domain. Real *platform*
  URLs that are structural, not personal, are fine where a collector genuinely
  targets them (`news.ycombinator.com/item?id=…`, `reddit.com/r/…`,
  `github.com/<fictional-org>/<repo>`).
- The backend seed and the frontend demo fixtures **mirror each other** — same
  workspaces, monitors, brands, and mention themes. Change one, change the other.

## Scenario 1 — Nimbus (primary; the demo workspace's own product)

A company using Pulp to monitor mentions of the developer-infra product it makes.

| Field | Value |
| --- | --- |
| Company | **Nimbus Labs** |
| Product | **Nimbus** — a self-hostable product-analytics / time-series database |
| Engine / package / binary | **nimbusdb** (lowercase; appears in bug reports) |
| Repos | `nimbus-labs/nimbus`, `nimbus-labs/nimbus-go` (client driver) |
| Domains | `nimbus.example`, `nimbuslabs.example` |
| Competitor (fictional) | **Orrery** — a hosted analytics DB, for comparison mentions |
| Channels | Hacker News; Reddit (`r/selfhosted`, `r/dataengineering`, `r/devops`, `r/programming`); GitHub |
| Monitors | Brand (`Nimbus`, `Nimbus Labs`) · Product (`nimbusdb`, excludes `hiring`/`job`) · Competitor watch (`Orrery`, AI-filtered to comparisons) |
| Mention themes | benchmarks & perf, self-hosting setup help, feature requests, pricing/licensing confusion, an outage gripe, `Nimbus vs Orrery` comparisons, a recruiting-post that the AI filter rejects |

## Scenario 2 — Fern (secondary; an open-source project a maintainer watches)

A solo maintainer using Pulp to keep up with buzz, questions, and bug reports
about their open-source project.

| Field | Value |
| --- | --- |
| Project | **Fern** — an open-source static-site generator |
| Package / binary | **fern** (CLI); package name `fern-ssg` |
| Repo | `fern-ssg/fern` |
| Domain | `fern.example` |
| Maintainer handle | `iris_fern` (fictional) |
| Alternative (fictional) | **Bramble** — another SSG, for comparison mentions |
| Channels | GitHub (issues/PRs/Show HN); Reddit (`r/webdev`, `r/javascript`); Hacker News (Show HN) |
| Monitors | Project mentions (`Fern`, `fern-ssg`) · Build/issue watch (repo-scoped to `fern-ssg/fern`) |
| Mention themes | build errors, plugin questions, `Fern vs Bramble`, a `Show HN` launch, feature requests, a showcase post, a low-quality/spam post the AI filter rejects |

When you add or edit any fixture, seed row, mock response, or doc example, use
only the entities in these two tables. If you need a new persona, invent another
obviously-fake handle in the same style — do not reach for a real one.
