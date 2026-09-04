use docmost_client::{
    ContentFormat, DocmostClient, DocmostError, ExportFormat, LoginRequest, PageRequest,
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

fn client(server: &MockServer) -> DocmostClient {
    DocmostClient::builder(&server.uri())
        .unwrap()
        .bearer_token(SecretString::from("token"))
        .build()
        .unwrap()
}

fn envelope(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200, "data": data}))
}

#[tokio::test]
async fn normalizes_url_and_sends_bearer_json_rpc_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/workspace/info"))
        .and(header("authorization", "Bearer token"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({})))
        .respond_with(envelope(json!({"id": "ws-1", "name": "Acme"})))
        .expect(2)
        .mount(&server)
        .await;
    for url in [server.uri(), format!("{}/api/", server.uri())] {
        let client = DocmostClient::builder(&url)
            .unwrap()
            .bearer_token(SecretString::from("token"))
            .build()
            .unwrap();
        assert_eq!(client.api_url().path(), "/api/");
        let info: Value = client.workspace().info().await.unwrap();
        assert_eq!(info["name"], "Acme");
    }
}

#[tokio::test]
async fn rejects_urls_with_credentials_or_query() {
    for url in [
        "https://user:pw@wiki.example.com",
        "https://wiki.example.com/?x=1",
        "ftp://wiki.example.com",
    ] {
        assert!(matches!(
            DocmostClient::builder(url),
            Err(DocmostError::InvalidUrl(_))
        ));
    }
}

#[tokio::test]
async fn login_reads_token_from_cookie_and_sends_no_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .and(body_json(
            json!({"email": "ada@example.com", "password": "secret"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "other=1; Path=/")
                .insert_header(
                    "set-cookie",
                    "authToken=jwt.session.token; Path=/; HttpOnly; SameSite=Lax",
                )
                .set_body_json(json!({"success": true, "status": 200})),
        )
        .mount(&server)
        .await;
    let client = DocmostClient::builder(&server.uri())
        .unwrap()
        .build()
        .unwrap();
    let token = client
        .auth()
        .login(&LoginRequest {
            email: "ada@example.com".into(),
            password: SecretString::from("secret"),
        })
        .await
        .unwrap();
    assert_eq!(token.expose_secret(), "jwt.session.token");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.get("authorization").is_none());
}

#[tokio::test]
async fn login_reports_mfa_accounts_and_missing_cookies() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(envelope(
            json!({"userHasMfa": true, "requiresMfaSetup": false, "isMfaEnforced": false}),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .mount(&server)
        .await;
    let client = DocmostClient::builder(&server.uri())
        .unwrap()
        .build()
        .unwrap();
    let request = LoginRequest {
        email: "ada@example.com".into(),
        password: SecretString::from("secret"),
    };
    assert!(matches!(
        client.auth().login(&request).await,
        Err(DocmostError::MfaRequired)
    ));
    assert!(matches!(
        client.auth().login(&request).await,
        Err(DocmostError::MissingAuthCookie)
    ));
}

#[tokio::test]
async fn maps_status_codes_and_validation_messages() {
    let server = MockServer::start().await;
    let cases = [
        (
            "/api/pages/info",
            401,
            json!({"message": "Unauthorized", "statusCode": 401}),
        ),
        (
            "/api/pages/create",
            400,
            json!({"message": ["spaceId must be a UUID"], "error": "Bad Request", "statusCode": 400}),
        ),
        (
            "/api/pages/delete",
            404,
            json!({"message": "Page not found", "error": "Not Found", "statusCode": 404}),
        ),
        (
            "/api/pages/update",
            413,
            json!({"message": "Request body is too large", "statusCode": 413}),
        ),
        (
            "/api/auth/logout",
            429,
            json!({"message": "ThrottlerException: Too Many Requests", "statusCode": 429}),
        ),
        (
            "/api/search",
            500,
            json!({"statusCode": 500, "message": "Internal server error"}),
        ),
    ];
    for (route, status, body) in &cases {
        Mock::given(method("POST"))
            .and(path(*route))
            .respond_with(
                ResponseTemplate::new(*status)
                    .insert_header("retry-after", "30")
                    .set_body_json(body),
            )
            .mount(&server)
            .await;
    }
    let client = client(&server);
    let unauthorized = client
        .pages()
        .info::<Value>("p1", None, false)
        .await
        .unwrap_err();
    assert!(matches!(unauthorized, DocmostError::Unauthorized { .. }));
    let validation = client
        .pages()
        .create::<Value, _>(&json!({}))
        .await
        .unwrap_err();
    match validation {
        DocmostError::ClientResponse { status, message } => {
            assert_eq!(status.as_u16(), 400);
            assert_eq!(message, "spaceId must be a UUID");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(matches!(
        client.pages().delete("p1", false).await.unwrap_err(),
        DocmostError::NotFound { message } if message == "Page not found"
    ));
    assert!(matches!(
        client
            .pages()
            .update::<Value, _>(&json!({}))
            .await
            .unwrap_err(),
        DocmostError::PayloadTooLarge { .. }
    ));
    assert!(matches!(
        client.auth().logout().await.unwrap_err(),
        DocmostError::RateLimited { retry_after: Some(after), .. } if after == "30"
    ));
    assert!(matches!(
        client
            .search()
            .search::<Value, _>(&json!({}))
            .await
            .unwrap_err(),
        DocmostError::ServerResponse { .. }
    ));
}

#[tokio::test]
async fn list_merges_cursor_pagination_and_follows_next_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/spaces"))
        .and(body_json(json!({"limit": 2, "query": "eng"})))
        .respond_with(envelope(json!({
            "items": [{"id": "s1"}, {"id": "s2"}],
            "meta": {"limit": 2, "hasNextPage": true, "hasPrevPage": false, "nextCursor": "c2", "prevCursor": null}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/spaces"))
        .and(body_json(json!({"limit": 2, "query": "eng", "cursor": "c2"})))
        .respond_with(envelope(json!({
            "items": [{"id": "s3"}],
            "meta": {"limit": 2, "hasNextPage": false, "hasPrevPage": true, "nextCursor": null, "prevCursor": "c1"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let page = PageRequest {
        limit: Some(2),
        query: Some("eng".into()),
        ..PageRequest::default()
    };
    let all = client(&server)
        .spaces()
        .list::<Value>(&page, true)
        .await
        .unwrap();
    let ids: Vec<&str> = all.items.iter().filter_map(|i| i["id"].as_str()).collect();
    assert_eq!(ids, ["s1", "s2", "s3"]);
    assert_eq!(all.meta.unwrap().has_next_page, Some(false));
    server.verify().await;
}

#[tokio::test]
async fn page_info_requests_content_in_the_chosen_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/info"))
        .and(body_json(json!({"pageId": "slug123", "includeContent": true, "includeSpace": true, "format": "markdown"})))
        .respond_with(envelope(json!({"id": "p1", "slugId": "slug123", "content": "# Title"})))
        .mount(&server)
        .await;
    let page: Value = client(&server)
        .pages()
        .info("slug123", Some(ContentFormat::Markdown), true)
        .await
        .unwrap();
    assert_eq!(page["content"], "# Title");
}

#[tokio::test]
async fn upload_sends_multipart_fields_and_returns_raw_attachment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/files/upload"))
        .and(header("authorization", "Bearer token"))
        .and(|request: &Request| {
            let content_type = request
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            let body = String::from_utf8_lossy(&request.body);
            content_type.starts_with("multipart/form-data")
                && body.contains("name=\"pageId\"\r\n\r\npage-1")
                && body.contains("name=\"file\"; filename=\"diagram.png\"")
                && body.contains("Content-Type: image/png")
                && body.contains("PNGDATA")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "att-1", "fileName": "diagram.png", "mimeType": "image/png", "fileSize": 7,
            "url": "http://localhost/api/files/att-1/diagram.png"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let attachment: docmost_client::Attachment = client(&server)
        .files()
        .upload("page-1", "diagram.png", "image/png", b"PNGDATA".to_vec())
        .await
        .unwrap();
    assert_eq!(attachment.id, "att-1");
    assert_eq!(attachment.file_name.as_deref(), Some("diagram.png"));
    server.verify().await;
}

#[tokio::test]
async fn export_returns_raw_bytes_and_file_name() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/export"))
        .and(body_json(json!({"pageId": "p1", "format": "markdown", "includeChildren": false, "includeAttachments": false})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/markdown")
                .insert_header("content-disposition", "attachment; filename=\"My%20Page.md\"")
                .set_body_bytes(b"# My Page\n".to_vec()),
        )
        .mount(&server)
        .await;
    let download = client(&server)
        .pages()
        .export("p1", ExportFormat::Markdown, false, false)
        .await
        .unwrap();
    assert_eq!(download.bytes, b"# My Page\n");
    assert_eq!(download.file_name.as_deref(), Some("My%20Page.md"));
    assert_eq!(download.content_type.as_deref(), Some("text/markdown"));
}

#[tokio::test]
async fn void_responses_without_data_are_accepted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/delete"))
        .and(body_json(
            json!({"pageId": "p1", "permanentlyDelete": true}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200})),
        )
        .mount(&server)
        .await;
    client(&server).pages().delete("p1", true).await.unwrap();
}

#[tokio::test]
async fn missing_token_fails_before_any_request() {
    let server = MockServer::start().await;
    let client = DocmostClient::builder(&server.uri())
        .unwrap()
        .build()
        .unwrap();
    assert!(matches!(
        client.workspace().info::<Value>().await,
        Err(DocmostError::Unauthorized { .. })
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}
