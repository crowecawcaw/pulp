//! Integration tests for the OpenAI-compatible AI judge against a mock
//! `/chat/completions` endpoint. The judge must map include -> 1.0 and
//! exclude -> 0.0, and fail closed (None) on malformed output, HTTP errors,
//! timeouts, and unreachable servers.

use std::time::Duration;

use httpmock::prelude::*;
use pulp::ai::local::OpenAiCompatJudge;
use pulp::ai::AiJudge;

/// An OpenAI chat-completions response whose assistant message carries the
/// given content string.
fn chat_response(content: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "model": "test-model",
        "choices": [
            { "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": "stop" }
        ]
    })
}

#[test]
fn include_verdict_maps_to_one() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            // The request must carry the configured model, non-streaming mode,
            // and the watch prompt embedded in the system message.
            .json_body_partial(r#"{ "model": "test-model", "stream": false }"#)
            .body_contains("desktop UI testing tools");
        then.status(200).json_body(chat_response(
            r#"{"verdict":"include","reason":"on topic"}"#,
        ));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    let score = judge.score(
        "desktop UI testing tools",
        "Looking for a way to automate WPF apps",
    );
    assert_eq!(score, Some(1.0));
    mock.assert();
}

#[test]
fn request_carries_no_think_hint() {
    // The outgoing request must disable "thinking" so reasoning models
    // (Qwen3.x via Ollama, …) answer directly instead of spending their whole
    // token budget on hidden reasoning and returning empty content.
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            .json_body_partial(r#"{ "reasoning_effort": "none" }"#);
        then.status(200)
            .json_body(chat_response(r#"{"verdict":"include","reason":"ok"}"#));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), Some(1.0));
    mock.assert();
}

#[test]
fn verdict_in_reasoning_field_is_recovered() {
    // Belt-and-suspenders: if a server ignores the no-think hint and leaves
    // `content` empty (budget spent thinking) but exposes a separate
    // `reasoning` field, the judge recovers the verdict from there rather than
    // failing closed.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning": "Let me think... the mention matches the context.\n{\"verdict\": \"include\", \"reason\": \"on topic\"}"
                },
                "finish_reason": "length"
            }]
        }));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), Some(1.0));
}

#[test]
fn empty_content_with_no_recoverable_reasoning_fails_closed() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "",
                    "reasoning": "Hmm, still pondering with no conclusion yet"
                },
                "finish_reason": "length"
            }]
        }));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), None);
}

#[test]
fn api_key_is_sent_as_bearer_token() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            .header("authorization", "Bearer sk-secret");
        then.status(200)
            .json_body(chat_response(r#"{"verdict":"include","reason":"ok"}"#));
    });

    let judge = OpenAiCompatJudge::new(
        format!("{}/v1", server.base_url()),
        "test-model",
        Some("sk-secret".to_string()),
    );
    assert_eq!(judge.score("prompt", "text"), Some(1.0));
    mock.assert();
}

#[test]
fn exclude_verdict_maps_to_zero() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(chat_response(
            r#"{"verdict":"exclude","reason":"web-only"}"#,
        ));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "Selenium for websites"), Some(0.0));
}

#[test]
fn chatter_around_json_still_parses() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(chat_response(
            "Sure!\n```json\n{\"verdict\": \"include\", \"reason\": \"on topic\"}\n```",
        ));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), Some(1.0));
}

#[test]
fn malformed_model_output_fails_closed() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200)
            .json_body(chat_response("sure, that looks relevant to me!"));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), None);
}

#[test]
fn unexpected_verdict_value_fails_closed() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200)
            .json_body(chat_response(r#"{"verdict":"maybe","reason":"unsure"}"#));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), None);
}

#[test]
fn malformed_response_envelope_fails_closed() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).body("not json at all");
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), None);
}

#[test]
fn http_error_fails_closed() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(500).body("model not found");
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None);
    assert_eq!(judge.score("prompt", "text"), None);
}

#[test]
fn timeout_fails_closed() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200)
            .json_body(chat_response(r#"{"verdict":"include","reason":"slow"}"#))
            .delay(Duration::from_secs(3));
    });

    let judge = OpenAiCompatJudge::new(format!("{}/v1", server.base_url()), "test-model", None)
        .with_timeout(Duration::from_millis(250));
    assert_eq!(judge.score("prompt", "text"), None);
}

#[test]
fn connection_refused_fails_closed() {
    // Bind an ephemeral port, then drop the listener so nothing is listening.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let judge = OpenAiCompatJudge::new(format!("http://127.0.0.1:{}/v1", port), "test-model", None)
        .with_timeout(Duration::from_secs(2));
    assert_eq!(judge.score("prompt", "text"), None);
}
