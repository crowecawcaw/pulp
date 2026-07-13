mod common;

async fn create_workspace(app: &common::TestApp, name: &str) -> String {
    let resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = resp.json().await.unwrap();
    ws["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_create_monitor() {
    let app = common::spawn_app().await;
    let ws_id = create_workspace(&app, "KW Workspace").await;

    let resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({
            "workspace_id": ws_id,
            "terms": ["rust lang", "rustlang"],
            "channels": ["hackernews", "github"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let kw: serde_json::Value = resp.json().await.unwrap();
    let terms = kw["terms"].as_array().unwrap();
    assert_eq!(terms.len(), 2);
    assert_eq!(terms[0].as_str().unwrap(), "rust lang");
    assert_eq!(terms[1].as_str().unwrap(), "rustlang");
    assert_eq!(kw["workspace_id"].as_str().unwrap(), ws_id);
    let channels = kw["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 2);
}

#[tokio::test]
async fn test_list_monitors_by_workspace() {
    let app = common::spawn_app().await;
    let ws1 = create_workspace(&app, "Workspace 1").await;
    let ws2 = create_workspace(&app, "Workspace 2").await;

    app.post("/api/monitors")
        .json(&serde_json::json!({ "workspace_id": ws1, "terms": ["alpha"] }))
        .send()
        .await
        .unwrap();
    app.post("/api/monitors")
        .json(&serde_json::json!({ "workspace_id": ws1, "terms": ["beta"] }))
        .send()
        .await
        .unwrap();
    app.post("/api/monitors")
        .json(&serde_json::json!({ "workspace_id": ws2, "terms": ["gamma"] }))
        .send()
        .await
        .unwrap();

    let resp1 = app
        .get(&format!("/api/monitors?workspace_id={}", ws1))
        .send()
        .await
        .unwrap();
    let kws1: serde_json::Value = resp1.json().await.unwrap();
    assert_eq!(kws1.as_array().unwrap().len(), 2);

    let resp2 = app
        .get(&format!("/api/monitors?workspace_id={}", ws2))
        .send()
        .await
        .unwrap();
    let kws2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(kws2.as_array().unwrap().len(), 1);
    assert_eq!(kws2[0]["terms"][0].as_str().unwrap(), "gamma");
}

#[tokio::test]
async fn test_update_monitor() {
    let app = common::spawn_app().await;
    let ws_id = create_workspace(&app, "Update WS").await;

    let create_resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({ "workspace_id": ws_id, "terms": ["original phrase"] }))
        .send()
        .await
        .unwrap();
    let kw: serde_json::Value = create_resp.json().await.unwrap();
    let kw_id = kw["id"].as_str().unwrap();

    let update_resp = app
        .put(&format!("/api/monitors/{}", kw_id))
        .json(&serde_json::json!({
            "terms": ["updated phrase", "second term"],
            "channels": ["reddit"],
            "active": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(update_resp.status(), 200);
    let updated: serde_json::Value = update_resp.json().await.unwrap();
    assert_eq!(updated["terms"][0].as_str().unwrap(), "updated phrase");
    assert_eq!(updated["terms"][1].as_str().unwrap(), "second term");
    assert_eq!(updated["active"].as_bool().unwrap(), false);
    let channels = updated["channels"].as_array().unwrap();
    assert_eq!(channels[0].as_str().unwrap(), "reddit");
}

#[tokio::test]
async fn test_delete_monitor() {
    let app = common::spawn_app().await;
    let ws_id = create_workspace(&app, "Del KW WS").await;

    let create_resp = app
        .post("/api/monitors")
        .json(&serde_json::json!({ "workspace_id": ws_id, "terms": ["to delete"] }))
        .send()
        .await
        .unwrap();
    let kw: serde_json::Value = create_resp.json().await.unwrap();
    let kw_id = kw["id"].as_str().unwrap();

    let del_resp = app
        .delete(&format!("/api/monitors/{}", kw_id))
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 204);

    let list_resp = app
        .get(&format!("/api/monitors?workspace_id={}", ws_id))
        .send()
        .await
        .unwrap();
    let list: serde_json::Value = list_resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 0);
}
