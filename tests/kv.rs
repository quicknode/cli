//! Integration tests for `qn kv …`.

mod common;

use common::run_qn;
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn set_put() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/kv/rest/v1/sets"))
        .and(body_json(json!({ "key": "k1", "value": "v1" })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["kv", "set", "put", "k1", "v1"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn set_get() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/kv/rest/v1/sets/k1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "value": "v1" } })),
        )
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["kv", "set", "get", "k1"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn set_ls() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/kv/rest/v1/sets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{ "key": "k1", "value": "v1" }],
            "cursor": ""
        })))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["kv", "set", "ls"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn set_delete_needs_yes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/kv/rest/v1/sets/k1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let no = run_qn(&server.uri(), &["kv", "set", "delete", "k1"]).await;
    assert_eq!(no.exit_code, 5);
    let yes = run_qn(&server.uri(), &["kv", "set", "delete", "k1", "--yes"]).await;
    assert_eq!(yes.exit_code, 0, "stderr={}", yes.stderr);
}

#[tokio::test]
async fn set_bulk_sends_payload() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/kv/rest/v1/sets/bulk"))
        .and(body_json(json!({
            "addSets": { "a": "1", "b": "2" },
            "deleteSets": ["old"]
        })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "kv", "set", "bulk", "--add", "a=1", "--add", "b=2", "--delete", "old",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn list_create_then_get_then_contains() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/kv/rest/v1/lists"))
        .and(body_json(json!({ "key": "lk", "items": ["x", "y"] })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/kv/rest/v1/lists/lk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "items": ["x", "y"] },
            "cursor": ""
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/kv/rest/v1/lists/lk/contains/x"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "exists": true } })),
        )
        .mount(&server)
        .await;

    assert_eq!(
        run_qn(&server.uri(), &["kv", "list", "create", "lk", "x", "y"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["kv", "list", "get", "lk"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["kv", "list", "contains", "lk", "x"])
            .await
            .exit_code,
        0
    );
}

#[tokio::test]
async fn list_update_send_add_remove() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/kv/rest/v1/lists/lk"))
        .and(body_json(json!({
            "addItems": ["new1"],
            "removeItems": ["old1"]
        })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(
        &server.uri(),
        &[
            "kv", "list", "update", "lk", "--add", "new1", "--remove", "old1",
        ],
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}

#[tokio::test]
async fn list_append_and_remove() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/kv/rest/v1/lists/lk/items"))
        .and(body_json(json!({ "item": "z" })))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/kv/rest/v1/lists/lk/items/z"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert_eq!(
        run_qn(&server.uri(), &["kv", "list", "append", "lk", "z"])
            .await
            .exit_code,
        0
    );
    assert_eq!(
        run_qn(&server.uri(), &["kv", "list", "remove-item", "lk", "z"])
            .await
            .exit_code,
        0
    );
}

#[tokio::test]
async fn list_delete() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/kv/rest/v1/lists/lk"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let out = run_qn(&server.uri(), &["kv", "list", "delete", "lk", "--yes"]).await;
    assert_eq!(out.exit_code, 0, "stderr={}", out.stderr);
}
