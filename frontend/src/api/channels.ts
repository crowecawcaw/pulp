// The channel names Pulp collects from. Mirrors the backend's canonical list
// (`collectors::CHANNELS` in `backend/src/collectors/mod.rs`) — collectors are
// Rust trait impls, not DB-driven config, so there is no way to derive this
// list from the API at build time; keep the two in sync by hand.
//
// The `GET /api/channels` response only carries per-channel *config* (a row
// exists once a channel has been touched), not the definitive set of channel
// *kinds* the backend can collect from — this constant is that set, used
// wherever the UI needs to enumerate all channels regardless of whether any
// have been configured yet.
export const CHANNELS = ['hackernews', 'reddit', 'github'] as const

export type Channel = (typeof CHANNELS)[number]
