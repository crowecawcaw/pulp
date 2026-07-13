mod common;

#[tokio::test]
async fn test_list_workspaces_empty() {
    let app = common::spawn_app().await;
    let resp = app.get("/api/workspaces").send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_create_workspace() {
    let app = common::spawn_app().await;
    let resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "Test WS", "description": "A test workspace" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "Test WS");
    assert_eq!(body["description"].as_str().unwrap(), "A test workspace");
    assert!(body["id"].as_str().is_some());

    // Verify it shows up in list
    let list_resp = app.get("/api/workspaces").send().await.unwrap();
    let list: serde_json::Value = list_resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_update_workspace() {
    let app = common::spawn_app().await;

    let create_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "Original" }))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = create_resp.json().await.unwrap();
    let id = ws["id"].as_str().unwrap();

    let update_resp = app
        .put(&format!("/api/workspaces/{}", id))
        .json(&serde_json::json!({ "name": "Updated", "description": "Updated desc" }))
        .send()
        .await
        .unwrap();
    assert_eq!(update_resp.status(), 200);
    let updated: serde_json::Value = update_resp.json().await.unwrap();
    assert_eq!(updated["name"].as_str().unwrap(), "Updated");
    assert_eq!(updated["description"].as_str().unwrap(), "Updated desc");
}

#[tokio::test]
async fn test_delete_workspace() {
    let app = common::spawn_app().await;

    let create_resp = app
        .post("/api/workspaces")
        .json(&serde_json::json!({ "name": "ToDelete" }))
        .send()
        .await
        .unwrap();
    let ws: serde_json::Value = create_resp.json().await.unwrap();
    let id = ws["id"].as_str().unwrap();

    let del_resp = app
        .delete(&format!("/api/workspaces/{}", id))
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 204);

    // Should be gone
    let list_resp = app.get("/api/workspaces").send().await.unwrap();
    let list: serde_json::Value = list_resp.json().await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 0);
}
