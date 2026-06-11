use http_body_util::BodyExt;
use tower::ServiceExt;

type Body = http_body_util::Full<bytes::Bytes>;

fn req(method: &str, path: &str, body: &str) -> http::Request<Body> {
    http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(bytes::Bytes::from(body.to_owned())))
        .unwrap()
}

async fn body_string(resp: http::Response<http_body_util::Full<bytes::Bytes>>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn create_entity_add_edit_delete() {
    // Shared in-memory DB across requests via a single connection wrapped per request.
    // ecsdb in-memory is per-connection, so open a file-backed temp db instead.
    let dir = std::env::temp_dir().join(format!("ecsdb_web_smoke_{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let path = dir.clone();

    let make_service = || {
        let path = path.clone();
        ecsdb_web::service::<Body, _>("/", move |_req| ecsdb::Ecs::open(&path))
    };

    // 1. Create an entity with a JSON component.
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            "/entities",
            "component_name=smoke::Foo&component_data=%7B%22n%22%3A1%7D",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let entity_path = location.trim_start_matches('/').to_owned();
    assert!(entity_path.starts_with("entities/"), "got {location}");
    let entity_id: i64 = entity_path.rsplit('/').next().unwrap().parse().unwrap();

    // 2. View the entity, confirm component present.
    let svc = make_service();
    let resp = svc
        .oneshot(req("GET", &format!("/{entity_path}"), ""))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let html = body_string(resp).await;
    assert!(html.contains("smoke::Foo"), "missing component: {html}");
    assert!(html.contains("Add component"), "missing add link");

    // 3. Add a second component.
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            &format!("/entities/{entity_id}/components"),
            "component_name=smoke::Bar&component_data=%22hello%22",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::SEE_OTHER);

    // 4. Edit the second component.
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            &format!("/entities/{entity_id}/components/smoke::Bar"),
            "component_data=%22world%22",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::SEE_OTHER);

    // 5. Confirm both components and edited value.
    let svc = make_service();
    let resp = svc
        .oneshot(req("GET", &format!("/entities/{entity_id}"), ""))
        .await
        .unwrap();
    let html = body_string(resp).await;
    assert!(html.contains("smoke::Foo"));
    assert!(html.contains("smoke::Bar"));
    assert!(html.contains("world"), "edit not applied: {html}");

    // 6. Delete a component via the no-JS POST fallback (HTML forms can't DELETE).
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            &format!("/entities/{entity_id}/components/smoke::Bar/delete"),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::SEE_OTHER);

    let svc = make_service();
    let resp = svc
        .oneshot(req("GET", &format!("/entities/{entity_id}"), ""))
        .await
        .unwrap();
    let html = body_string(resp).await;
    assert!(!html.contains("smoke::Bar"), "delete failed: {html}");
    assert!(html.contains("smoke::Foo"));

    // 7. New-entity form renders with datalist of known components.
    let svc = make_service();
    let resp = svc.oneshot(req("GET", "/entities/new", "")).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let html = body_string(resp).await;
    assert!(html.contains("known-components"), "no datalist: {html}");

    // 8. Empty component name is rejected.
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            "/entities",
            "component_name=&component_data=%7B%7D",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_file(&dir);
}

/// Regression: `component_data` is the raw JSON text of the value. It must be
/// parsed as JSON, not wrapped in a JSON string. Otherwise an object like
/// `{"n":1}` round-trips as the double-quoted string `"{\"n\":1}"`.
#[tokio::test(flavor = "multi_thread")]
async fn json_component_data_is_not_double_quoted() {
    let dir = std::env::temp_dir().join(format!("ecsdb_web_json_{}", std::process::id()));
    let _ = std::fs::remove_file(&dir);
    let path = dir.clone();

    let make_service = || {
        let path = path.clone();
        ecsdb_web::service::<Body, _>("/", move |_req| ecsdb::Ecs::open(&path))
    };

    // Create an entity whose component holds a JSON object.
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            "/entities",
            // component_data = {"n":1,"s":"hi"}
            "component_name=json::Obj&component_data=%7B%22n%22%3A1%2C%22s%22%3A%22hi%22%7D",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::SEE_OTHER);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    let entity_id: i64 = location.rsplit('/').next().unwrap().parse().unwrap();

    // Open the component editor and inspect the textarea contents.
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "GET",
            &format!("/entities/{entity_id}/components/json::Obj"),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::OK);
    let html = body_string(resp).await;

    // The editor must show the object fields as JSON (maud HTML-escapes `"` as
    // `&quot;`), not an escaped/quoted string.
    assert!(
        html.contains("&quot;n&quot;: 1") && html.contains("&quot;s&quot;: &quot;hi&quot;"),
        "editor should show parsed JSON object, got: {html}"
    );
    // The tell-tale of double-quoting is the whole object wrapped in a string,
    // i.e. the textarea starting with `&quot;{` and containing escaped quotes.
    assert!(
        !html.contains(r#"&quot;{\&quot;"#) && !html.contains(r#">&quot;{"#),
        "JSON was double-quoted/escaped: {html}"
    );

    // Editing with a bare string value must store a JSON string, and round-trip
    // as exactly that string (no extra quoting layer).
    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "POST",
            &format!("/entities/{entity_id}/components/json::Obj"),
            // component_data = "hello"
            "component_data=%22hello%22",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), http::StatusCode::SEE_OTHER);

    let svc = make_service();
    let resp = svc
        .oneshot(req(
            "GET",
            &format!("/entities/{entity_id}/components/json::Obj"),
            "",
        ))
        .await
        .unwrap();
    let html = body_string(resp).await;
    // Textarea should contain exactly "hello" (one quoting layer, HTML-escaped),
    // not a double-quoted "\"hello\"".
    assert!(
        html.contains(">&quot;hello&quot;</textarea>"),
        "string value should round-trip without extra quoting: {html}"
    );
    assert!(
        !html.contains(r#"\&quot;hello\&quot;"#),
        "string value was double-quoted: {html}"
    );

    let _ = std::fs::remove_file(&dir);
}
