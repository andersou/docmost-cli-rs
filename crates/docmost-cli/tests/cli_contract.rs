use std::fs;

use assert_cmd::Command;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::tempdir;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

#[test]
fn help_exposes_requested_command_groups() {
    let mut command = Command::cargo_bin("docmost-cli").unwrap();
    command
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("workspace"))
        .stdout(contains("space"))
        .stdout(contains("page"))
        .stdout(contains("comment"))
        .stdout(contains("search"));
}

#[test]
fn delete_requires_explicit_confirmation_before_network() {
    let directory = tempdir().unwrap();
    let config = directory.path().join("missing.json");
    docmost(&config, &["page", "delete", "p1"])
        .assert()
        .code(2)
        .stderr(contains("delete requires --yes"));
    docmost(&config, &["comment", "delete", "c1"])
        .assert()
        .code(2)
        .stderr(contains("delete requires --yes"));
}

#[test]
fn missing_url_and_missing_session_fail_without_network() {
    let directory = tempdir().unwrap();
    let config = directory.path().join("missing.json");
    docmost(&config, &["space", "list"])
        .assert()
        .code(3)
        .stderr(contains("Docmost URL required"));
    docmost(
        &config,
        &["--api-url", "http://127.0.0.1:9", "space", "list"],
    )
    .assert()
    .code(5)
    .stderr(contains("no valid session, run `docmost-cli auth login`"));
}

fn write_config(path: &std::path::Path, value: Value) {
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn read_config(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn docmost(config: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::cargo_bin("docmost-cli").unwrap();
    command
        .args(["--output", "json"])
        .args(args)
        .env("DOCMOST_CONFIG", config)
        .env_remove("DOCMOST_API_URL")
        .env_remove("DOCMOST_AUTH_TOKEN")
        .env_remove("DOCMOST_EMAIL")
        .env_remove("DOCMOST_PASSWORD")
        .env_remove("DOCMOST_PASSWORD_STORE_FILE");
    command
}

fn status_command(config: &std::path::Path) -> Command {
    docmost(config, &["auth", "status"])
}

/// Unsigned JWT whose payload carries the given `exp`; the CLI only reads
/// the claim, so the signature is irrelevant.
fn jwt(exp: u64) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let payload = format!(r#"{{"type":"access","exp":{exp}}}"#);
    let mut encoded = String::new();
    for chunk in payload.as_bytes().chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let n = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
        for i in 0..chunk.len() + 1 {
            encoded.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    format!("eyJhbGciOiJIUzI1NiJ9.{encoded}.sig")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn envelope(data: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200, "data": data}))
}

fn unauthorized() -> ResponseTemplate {
    ResponseTemplate::new(401).set_body_json(json!({"message": "Unauthorized", "statusCode": 401}))
}

async fn mount_identity(server: &MockServer, bearer: &str) {
    Mock::given(method("POST"))
        .and(path("/api/users/me"))
        .and(header("authorization", format!("Bearer {bearer}").as_str()))
        .respond_with(envelope(
            json!({"user": {"id": "u1", "email": "ada@example.com"}, "workspace": {"id": "w1"}}),
        ))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_login(server: &MockServer, email: &str, password: &str, token: &str) {
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .and(body_json(json!({"email": email, "password": password})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "set-cookie",
                    format!("authToken={token}; Path=/; HttpOnly; SameSite=Lax").as_str(),
                )
                .set_body_json(json!({"success": true, "status": 200})),
        )
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn login_stores_cookie_token_email_and_url_with_private_permissions() {
    let server = MockServer::start().await;
    mount_login(&server, "ada@example.com", "s3cret", "login-token").await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("nested").join("config.json");

    let mut command = docmost(
        &config,
        &[
            "--api-url",
            &server.uri(),
            "auth",
            "login",
            "--email",
            "ada@example.com",
            "--password-stdin",
        ],
    );
    command.write_stdin("s3cret\n");
    command
        .assert()
        .success()
        .stdout(contains("\"password_stored\": false"));
    assert_eq!(
        read_config(&config),
        json!({
            "api_url": server.uri(),
            "auth_token": "login-token",
            "email": "ada@example.com",
        })
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    server.verify().await;
}

#[tokio::test]
async fn login_with_remember_saves_the_password_in_the_store() {
    let server = MockServer::start().await;
    mount_login(&server, "ada@example.com", "s3cret", "login-token").await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    let store = directory.path().join("store.json");

    let mut command = docmost(
        &config,
        &["--api-url", &server.uri(), "auth", "login", "--remember"],
    );
    command
        .env("DOCMOST_EMAIL", "ada@example.com")
        .env("DOCMOST_PASSWORD", "s3cret")
        .env("DOCMOST_PASSWORD_STORE_FILE", &store);
    command
        .assert()
        .success()
        .stdout(contains("\"password_stored\": true"));
    assert_eq!(read_config(&config)["remember_password"], true);
    let entries: Value = serde_json::from_slice(&fs::read(&store).unwrap()).unwrap();
    assert!(
        entries
            .as_object()
            .unwrap()
            .values()
            .any(|password| password == "s3cret")
    );
    server.verify().await;
}

#[tokio::test]
async fn mfa_accounts_fail_login_with_a_clear_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(envelope(
            json!({"userHasMfa": true, "requiresMfaSetup": false, "isMfaEnforced": false}),
        ))
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut command = docmost(&config, &["--api-url", &server.uri(), "auth", "login"]);
    command
        .env("DOCMOST_EMAIL", "ada@example.com")
        .env("DOCMOST_PASSWORD", "s3cret");
    command
        .assert()
        .code(5)
        .stderr(contains("multi-factor authentication"));
    assert!(!config.exists());
}

#[tokio::test]
async fn expired_jwt_session_relogs_in_without_a_failing_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/me"))
        .and(header(
            "authorization",
            format!("Bearer {}", jwt(now() - 100)).as_str(),
        ))
        .respond_with(unauthorized())
        .expect(0)
        .mount(&server)
        .await;
    mount_login(&server, "ada@example.com", "env-password", "fresh-token").await;
    mount_identity(&server, "fresh-token").await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    write_config(
        &config,
        json!({"api_url": server.uri(), "auth_token": jwt(now() - 100), "email": "ada@example.com"}),
    );
    let original = fs::read(&config).unwrap();

    let mut command = status_command(&config);
    command.env("DOCMOST_PASSWORD", "env-password");
    command
        .assert()
        .success()
        .stdout(contains("\"email\": \"ada@example.com\""));
    // A password from the environment never persists the new session.
    assert_eq!(fs::read(&config).unwrap(), original);
    server.verify().await;
}

#[tokio::test]
async fn revoked_session_relogs_in_with_the_stored_password_and_persists() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/me"))
        .and(header("authorization", "Bearer revoked-token"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    mount_login(&server, "ada@example.com", "stored-password", "fresh-token").await;
    mount_identity(&server, "fresh-token").await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    let store = directory.path().join("store.json");
    write_config(
        &config,
        json!({
            "api_url": server.uri(),
            "auth_token": "revoked-token",
            "email": "ada@example.com",
            "remember_password": true,
        }),
    );
    fs::write(
        &store,
        serde_json::to_vec(&json!({
            format!("docmost-cli:{}\u{1f}ada@example.com", server.uri()): "stored-password",
        }))
        .unwrap(),
    )
    .unwrap();

    let mut command = status_command(&config);
    command.env("DOCMOST_PASSWORD_STORE_FILE", &store);
    command
        .assert()
        .success()
        .stdout(contains("\"password_stored\": true"));
    assert_eq!(
        read_config(&config),
        json!({
            "api_url": server.uri(),
            "auth_token": "fresh-token",
            "email": "ada@example.com",
            "remember_password": true,
        })
    );
    server.verify().await;
}

#[tokio::test]
async fn environment_access_token_does_not_fall_back_to_stored_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/me"))
        .and(header("authorization", "Bearer env-token"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    let store = directory.path().join("store.json");
    write_config(
        &config,
        json!({
            "api_url": server.uri(),
            "auth_token": "stored-token",
            "email": "ada@example.com",
            "remember_password": true,
        }),
    );
    fs::write(
        &store,
        serde_json::to_vec(&json!({
            format!("docmost-cli:{}\u{1f}ada@example.com", server.uri()): "stored-password",
        }))
        .unwrap(),
    )
    .unwrap();
    let original = fs::read(&config).unwrap();

    let mut command = status_command(&config);
    command
        .env("DOCMOST_AUTH_TOKEN", "env-token")
        .env("DOCMOST_PASSWORD_STORE_FILE", &store);
    command
        .assert()
        .code(5)
        .stderr(contains("authentication failed: Unauthorized"));
    assert_eq!(fs::read(&config).unwrap(), original);
    server.verify().await;
}

#[tokio::test]
async fn api_override_does_not_use_stored_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/me"))
        .and(header("authorization", "Bearer stored-token"))
        .respond_with(unauthorized())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    let store = directory.path().join("store.json");
    write_config(
        &config,
        json!({
            "api_url": "https://other.example/api",
            "auth_token": "stored-token",
            "email": "ada@example.com",
            "remember_password": true,
        }),
    );
    fs::write(
        &store,
        serde_json::to_vec(&json!({
            "docmost-cli:https://other.example/api\u{1f}ada@example.com": "stored-password",
        }))
        .unwrap(),
    )
    .unwrap();
    let original = fs::read(&config).unwrap();

    let mut command = status_command(&config);
    command
        .args(["--api-url", &server.uri()])
        .env("DOCMOST_PASSWORD_STORE_FILE", &store);
    command.assert().code(5);
    assert_eq!(fs::read(&config).unwrap(), original);
    server.verify().await;
}

#[tokio::test]
async fn logout_revokes_the_session_and_clears_local_state() {
    let server = MockServer::start().await;
    let token = jwt(now() + 3600);
    Mock::given(method("POST"))
        .and(path("/api/auth/logout"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = directory.path().join("config.json");
    let store = directory.path().join("store.json");
    write_config(
        &config,
        json!({
            "api_url": server.uri(),
            "auth_token": token,
            "email": "ada@example.com",
            "remember_password": true,
        }),
    );
    fs::write(
        &store,
        serde_json::to_vec(&json!({
            format!("docmost-cli:{}\u{1f}ada@example.com", server.uri()): "stored-password",
        }))
        .unwrap(),
    )
    .unwrap();

    let mut command = docmost(&config, &["auth", "logout"]);
    command.env("DOCMOST_PASSWORD_STORE_FILE", &store);
    command
        .assert()
        .success()
        .stdout(contains("\"session_revoked\": true"));
    assert_eq!(
        read_config(&config),
        json!({"api_url": server.uri(), "auth_token": null})
    );
    let entries: Value = serde_json::from_slice(&fs::read(&store).unwrap()).unwrap();
    assert!(entries.as_object().unwrap().is_empty());
    server.verify().await;
}

fn authenticated_config(directory: &std::path::Path, server: &MockServer) -> std::path::PathBuf {
    let config = directory.join("config.json");
    write_config(
        &config,
        json!({"api_url": server.uri(), "auth_token": "token", "email": "ada@example.com"}),
    );
    config
}

#[tokio::test]
async fn page_create_preprocesses_status_tags_and_sends_markdown() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/create"))
        .and(header("authorization", "Bearer token"))
        .and(body_json(json!({
            "spaceId": "space-1",
            "title": "Release",
            "parentPageId": "parent-1",
            "icon": "🚀",
            "content": "# Hi\n\n<span data-type=\"status\" data-color=\"green\">READY</span>\n",
            "format": "markdown",
            "coverPhoto": "x",
        })))
        .respond_with(envelope(json!({"id": "page-1", "title": "Release"})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);
    let content = directory.path().join("release.md");
    fs::write(&content, "# Hi\n\n<status color=\"green\">READY</status>\n").unwrap();

    docmost(
        &config,
        &[
            "page",
            "create",
            "--space",
            "space-1",
            "--title",
            "Release",
            "--parent",
            "parent-1",
            "--icon",
            "🚀",
            "--content-file",
            content.to_str().unwrap(),
            "--data",
            r#"{"coverPhoto":"x"}"#,
        ],
    )
    .assert()
    .success()
    .stdout(contains("\"id\": \"page-1\""));
    server.verify().await;
}

#[tokio::test]
async fn page_create_uploads_local_files_before_writing_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/create"))
        .and(body_json(json!({"spaceId": "space-1", "title": "Docs"})))
        .respond_with(envelope(json!({"id": "page-1"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/files/upload"))
        .and(|request: &Request| {
            let body = String::from_utf8_lossy(&request.body);
            body.contains("name=\"pageId\"\r\n\r\npage-1")
                && body.contains("filename=\"diagram.png\"")
                && body.contains("Content-Type: image/png")
                && body.contains("PNGDATA")
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "att-1", "fileName": "diagram.png", "mimeType": "image/png", "fileSize": 7
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/update"))
        .and(body_json(json!({
            "pageId": "page-1",
            "content": "See ![diagram](/api/files/att-1/diagram.png) and [web](https://example.com)\n",
            "operation": "replace",
            "format": "markdown",
        })))
        .respond_with(envelope(json!({"id": "page-1", "title": "Docs"})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);
    let docs = directory.path().join("docs");
    fs::create_dir_all(docs.join("img")).unwrap();
    fs::write(docs.join("img/diagram.png"), b"PNGDATA").unwrap();
    let content = docs.join("page.md");
    fs::write(
        &content,
        "See ![diagram](img/diagram.png) and [web](https://example.com)\n",
    )
    .unwrap();

    docmost(
        &config,
        &[
            "page",
            "create",
            "--space",
            "space-1",
            "--title",
            "Docs",
            "--content-file",
            content.to_str().unwrap(),
            "--upload-local-files",
        ],
    )
    .assert()
    .success()
    .stdout(contains("\"uploads\""))
    .stdout(contains("\"id\": \"att-1\""));
    server.verify().await;
}

#[tokio::test]
async fn page_edit_appends_content_from_stdin() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/update"))
        .and(body_json(json!({
            "pageId": "page-1",
            "title": "New title",
            "content": "## Appendix\n",
            "operation": "append",
            "format": "markdown",
        })))
        .respond_with(envelope(json!({"id": "page-1", "title": "New title"})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    let mut command = docmost(
        &config,
        &[
            "page",
            "edit",
            "page-1",
            "--title",
            "New title",
            "--content-file",
            "-",
            "--operation",
            "append",
        ],
    );
    command.write_stdin("## Appendix\n");
    command.assert().success();
    docmost(&config, &["page", "edit", "page-1"])
        .assert()
        .code(2)
        .stderr(contains("edit needs at least one field"));
    server.verify().await;
}

#[tokio::test]
async fn page_move_places_the_page_after_its_new_siblings() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/info"))
        .and(body_json(
            json!({"pageId": "page-9", "includeContent": true, "includeSpace": false}),
        ))
        .respond_with(envelope(
            json!({"id": "page-9", "spaceId": "space-1", "position": "a0"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/sidebar-pages"))
        .and(body_json(json!({"spaceId": "space-1", "pageId": "parent-1"})))
        .respond_with(envelope(json!({
            "items": [
                {"id": "page-2", "position": "a1V"},
                {"id": "page-9", "position": "a0"},
                {"id": "page-1", "position": "a0"},
            ],
            "meta": {"limit": 20, "hasNextPage": false, "hasPrevPage": false, "nextCursor": null, "prevCursor": null}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/move"))
        .and(body_json(
            json!({"pageId": "page-9", "parentPageId": "parent-1", "position": "a2VVV"}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(&config, &["page", "move", "page-9", "--parent", "parent-1"])
        .assert()
        .success()
        .stdout(contains("\"position\": \"a2VVV\""));
    server.verify().await;
}

#[tokio::test]
async fn page_move_with_explicit_position_skips_lookups() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/move"))
        .and(body_json(
            json!({"pageId": "page-9", "parentPageId": null, "position": "a0VVV"}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(
        &config,
        &["page", "move", "page-9", "--root", "--position", "a0VVV"],
    )
    .assert()
    .success();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn page_delete_reports_each_result_and_fails_on_partial_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/delete"))
        .and(body_json(
            json!({"pageId": "page-1", "permanentlyDelete": true}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "status": 200})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/delete"))
        .and(body_json(
            json!({"pageId": "page-2", "permanentlyDelete": true}),
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({"message": "Page not found", "error": "Not Found", "statusCode": 404}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(
        &config,
        &["page", "delete", "page-1", "page-2", "--yes", "--permanent"],
    )
    .assert()
    .code(5)
    .stdout(contains("\"deleted\": true"))
    .stdout(contains("Page not found"))
    .stderr(contains("1 of 2 deletions failed"));
    server.verify().await;
}

#[tokio::test]
async fn comment_create_wraps_text_into_a_document_string() {
    let server = MockServer::start().await;
    let document = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Looks good"}]}]}).to_string();
    Mock::given(method("POST"))
        .and(path("/api/comments/create"))
        .and(body_json(json!({
            "pageId": "page-1",
            "content": document,
            "selection": "second paragraph",
            "type": "inline",
            "parentCommentId": "comment-0",
        })))
        .respond_with(envelope(json!({"id": "comment-1"})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(
        &config,
        &[
            "comment",
            "create",
            "--page",
            "page-1",
            "--text",
            "Looks good",
            "--selection",
            "second paragraph",
            "--parent",
            "comment-0",
        ],
    )
    .assert()
    .success()
    .stdout(contains("\"id\": \"comment-1\""));
    server.verify().await;
}

#[tokio::test]
async fn page_export_streams_text_and_requires_out_for_archives() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/export"))
        .and(body_json(json!({"pageId": "page-1", "format": "markdown", "includeChildren": false, "includeAttachments": false})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/octet-stream")
                .insert_header(
                    "content-disposition",
                    "attachment; filename=\"My%20Page.md\"",
                )
                .set_body_bytes(b"# Page\n".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/export"))
        .and(body_json(json!({"pageId": "page-1", "format": "html", "includeChildren": true, "includeAttachments": false})))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/zip")
                .insert_header("content-disposition", "attachment; filename=\"Page.zip\"")
                .set_body_bytes(b"PK".to_vec()),
        )
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(&config, &["page", "export", "page-1"])
        .assert()
        .success()
        .stdout("# Page\n");
    docmost(
        &config,
        &[
            "page",
            "export",
            "page-1",
            "--format",
            "html",
            "--include-children",
        ],
    )
    .assert()
    .code(2)
    .stderr(contains("--out"));
    let out = directory.path().join("page.zip");
    docmost(
        &config,
        &[
            "page",
            "export",
            "page-1",
            "--format",
            "html",
            "--include-children",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .assert()
    .success()
    .stdout(contains("\"file_name\": \"Page.zip\""));
    assert_eq!(fs::read(out).unwrap(), b"PK");
}

#[tokio::test]
async fn list_all_follows_cursors_and_rejects_bad_limits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/spaces"))
        .and(body_json(json!({"limit": 1})))
        .respond_with(envelope(json!({
            "items": [{"id": "s1"}],
            "meta": {"limit": 1, "hasNextPage": true, "hasPrevPage": false, "nextCursor": "c2", "prevCursor": null}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/spaces"))
        .and(body_json(json!({"limit": 1, "cursor": "c2"})))
        .respond_with(envelope(json!({
            "items": [{"id": "s2"}],
            "meta": {"limit": 1, "hasNextPage": false, "hasPrevPage": true, "nextCursor": null, "prevCursor": "c1"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    let output = docmost(&config, &["space", "list", "--limit", "1", "--all"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["items"].as_array().unwrap().len(), 2);
    assert_eq!(value["meta"]["hasNextPage"], false);
    docmost(&config, &["space", "list", "--limit", "500"])
        .assert()
        .code(2)
        .stderr(contains("--limit must be between 1 and 100"));
    server.verify().await;
}

#[tokio::test]
async fn page_tree_recursive_walks_children_with_depth() {
    let server = MockServer::start().await;
    let meta = json!({"limit": 20, "hasNextPage": false, "hasPrevPage": false, "nextCursor": null, "prevCursor": null});
    Mock::given(method("POST"))
        .and(path("/api/pages/sidebar-pages"))
        .and(body_json(json!({"spaceId": "space-1"})))
        .respond_with(envelope(json!({
            "items": [{"id": "root-1", "hasChildren": true}, {"id": "root-2", "hasChildren": false}],
            "meta": meta,
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/sidebar-pages"))
        .and(body_json(json!({"spaceId": "space-1", "pageId": "root-1"})))
        .respond_with(envelope(json!({
            "items": [{"id": "child-1", "hasChildren": false}],
            "meta": meta,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    let output = docmost(
        &config,
        &["page", "tree", "--space", "space-1", "--recursive"],
    )
    .assert()
    .success()
    .get_output()
    .stdout
    .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    let flattened: Vec<(String, u64)> = value["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap().to_owned(),
                item["depth"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        flattened,
        [
            ("root-1".to_owned(), 0),
            ("child-1".to_owned(), 1),
            ("root-2".to_owned(), 0)
        ]
    );
    server.verify().await;
}

#[tokio::test]
async fn page_get_can_omit_content_and_choose_format() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/info"))
        .and(body_json(json!({"pageId": "slug-1", "includeContent": true, "includeSpace": false, "format": "markdown"})))
        .respond_with(envelope(json!({"id": "page-1", "content": "# Page"})))
        .expect(2)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(&config, &["page", "get", "slug-1"])
        .assert()
        .success()
        .stdout(contains("\"content\": \"# Page\""));
    let output = docmost(&config, &["page", "get", "slug-1", "--content", "markdown"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(output).unwrap().contains("# Page"));
    Mock::given(method("POST"))
        .and(path("/api/pages/info"))
        .and(body_json(
            json!({"pageId": "slug-1", "includeContent": true, "includeSpace": true}),
        ))
        .respond_with(envelope(
            json!({"id": "page-1", "content": {"type": "doc"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let output = docmost(
        &config,
        &[
            "page",
            "get",
            "slug-1",
            "--content",
            "none",
            "--include-space",
        ],
    )
    .assert()
    .success()
    .get_output()
    .stdout
    .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert!(value.get("content").is_none());
    server.verify().await;
}

#[tokio::test]
async fn page_url_builds_the_link_like_the_web_client() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/pages/info"))
        .and(body_json(json!({"pageId": "HGEzoeMw9o", "includeContent": true, "includeSpace": true})))
        .respond_with(envelope(json!({
            "id": "p1", "slugId": "HGEzoeMw9o", "title": "Plano de Teste — Migração InfluxDB → TDengine",
            "content": {"type": "doc"}, "space": {"id": "space-1", "slug": "general"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/info"))
        .and(body_json(
            json!({"pageId": "p2", "includeContent": true, "includeSpace": true}),
        ))
        .respond_with(envelope(json!({
            "id": "p2", "slugId": "bbbbbbbbbb", "title": "", "spaceId": "space-2",
            "content": {"type": "doc"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/spaces/info"))
        .and(body_json(json!({"spaceId": "space-2"})))
        .respond_with(envelope(json!({"id": "space-2", "slug": "docs"})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    let output = docmost(&config, &["page", "url", "HGEzoeMw9o"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value["url"],
        format!(
            "{}/s/general/p/plano-de-teste-migracao-influx-db-t-dengine-HGEzoeMw9o",
            server.uri()
        )
    );
    assert_eq!(value["spaceSlug"], "general");
    // Without an embedded space the slug is looked up by spaceId.
    let output = docmost(&config, &["page", "url", "p2"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        value["url"],
        format!("{}/s/docs/p/untitled-bbbbbbbbbb", server.uri())
    );
    server.verify().await;
}

#[tokio::test]
async fn page_attach_uploads_and_appends_nodes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/files/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "att-1", "fileName": "spec.pdf", "mimeType": "application/pdf", "fileSize": 3
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/update"))
        .and(body_json(json!({
            "pageId": "page-1",
            "content": {"type": "doc", "content": [{
                "type": "attachment",
                "attrs": {"url": "/api/files/att-1/spec.pdf", "name": "spec.pdf", "mime": "application/pdf", "size": 3, "attachmentId": "att-1"}
            }]},
            "operation": "append",
            "format": "json",
        })))
        .respond_with(envelope(json!({"id": "page-1"})))
        .expect(1)
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);
    let file = directory.path().join("spec.pdf");
    fs::write(&file, b"PDF").unwrap();

    docmost(
        &config,
        &["page", "attach", "page-1", "--file", file.to_str().unwrap()],
    )
    .assert()
    .success()
    .stdout(contains("\"inserted\": true"));
    server.verify().await;
}

#[tokio::test]
async fn rate_limits_and_oversized_bodies_have_dedicated_messages() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/search"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(json!({"message": "Too Many Requests", "statusCode": 429})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/update"))
        .respond_with(
            ResponseTemplate::new(413)
                .set_body_json(json!({"message": "Request body is too large", "statusCode": 413})),
        )
        .mount(&server)
        .await;
    let directory = tempdir().unwrap();
    let config = authenticated_config(directory.path(), &server);

    docmost(&config, &["search", "anything"])
        .assert()
        .code(7)
        .stderr(contains("rate limit"));
    docmost(&config, &["page", "edit", "page-1", "--content", "big"])
        .assert()
        .code(5)
        .stderr(contains("use `page import` for large files"));
}
