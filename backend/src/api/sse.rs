use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
};
use futures::stream::Stream;
use serde::Deserialize;
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::db::repos::traits::Mention;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SseQuery {
    pub workspace_id: Option<String>,
}

pub async fn stream(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SseQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.sse_tx.subscribe();
    let workspace_id = q.workspace_id;

    // Broadcast payloads are serialized `Mention`s, which carry `monitor_id`
    // but not `workspace_id` directly, so a scoped subscriber needs an async
    // lookup of the mention's monitor to resolve which workspace it belongs
    // to before deciding whether to forward it.
    let stream = BroadcastStream::new(rx)
        .filter_map(|msg| msg.ok())
        .then(move |data| {
            let state = state.clone();
            let workspace_id = workspace_id.clone();
            async move {
                let Some(ws_id) = workspace_id else {
                    return Some(data);
                };
                let mention: Option<Mention> = serde_json::from_str(&data).ok();
                let belongs = match mention {
                    Some(m) => state
                        .monitors
                        .get(&m.monitor_id)
                        .await
                        .ok()
                        .flatten()
                        .is_some_and(|mon| mon.workspace_id == ws_id),
                    None => false,
                };
                belongs.then_some(data)
            }
        })
        .filter_map(|data| data.map(|d| Ok(Event::default().event("mention").data(d))));

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    )
}
