use crate::db::repos::traits::Mention;

pub fn format_webhook_payload(mention: &Mention) -> serde_json::Value {
    serde_json::json!({
        "id": mention.id,
        "channel": mention.channel,
        "content_url": mention.content_url,
        "content_text": mention.content_text,
        "author_name": mention.author_name,
        "published_at": mention.published_at,
    })
}
