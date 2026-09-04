use std::{collections::BTreeMap, fmt, time::Duration};

use reqwest::{Method, StatusCode, header, multipart};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;
use url::Url;

/// Timeout for the JSON RPC calls that make up most of the API.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for uploads, downloads, and exports, which move whole files.
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug)]
pub struct LoginRequest {
    pub email: String,
    pub password: SecretString,
}

/// Content encodings accepted by the page create/update/info endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentFormat {
    Json,
    Markdown,
    Html,
}
impl ContentFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Html => "html",
        }
    }
}
impl fmt::Display for ContentFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How `pages/update` combines new content with the existing document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentOperation {
    Replace,
    Append,
    Prepend,
}
impl ContentOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Append => "append",
            Self::Prepend => "prepend",
        }
    }
}

/// Formats produced by the page and space export endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Markdown,
    Html,
}
impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Html => "html",
        }
    }
}

/// Cursor pagination fields shared by every Docmost list endpoint.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMeta {
    pub limit: Option<u64>,
    pub has_next_page: Option<bool>,
    pub has_prev_page: Option<bool>,
    pub next_cursor: Option<String>,
    pub prev_cursor: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListResponse<T> {
    pub items: Vec<T>,
    pub meta: Option<PageMeta>,
}

/// A raw file body returned by download and export endpoints.
#[derive(Clone, Debug)]
pub struct Download {
    pub content_type: Option<String>,
    pub file_name: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum DocmostError {
    #[error("invalid Docmost API URL: {0}")]
    InvalidUrl(String),
    #[error("HTTP transport failure: {message}")]
    Transport { message: String },
    #[error("response decoding failure: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("authentication failed: {message}")]
    Unauthorized { message: String },
    #[error("permission denied: {message}")]
    Forbidden { message: String },
    #[error("resource not found: {message}")]
    NotFound { message: String },
    #[error("request too large: {message}")]
    PayloadTooLarge { message: String },
    #[error("Docmost rate limit reached: {message}")]
    RateLimited {
        message: String,
        retry_after: Option<String>,
    },
    #[error("Docmost client error {status}: {message}")]
    ClientResponse { status: StatusCode, message: String },
    #[error("Docmost server error {status}: {message}")]
    ServerResponse { status: StatusCode, message: String },
    #[error("unexpected HTTP status {status}: {message}")]
    UnexpectedStatus { status: StatusCode, message: String },
    #[error(
        "this account requires multi-factor authentication, which the API login does not support"
    )]
    MfaRequired,
    #[error("login succeeded but the server sent no authToken cookie")]
    MissingAuthCookie,
}

#[derive(Clone)]
pub struct DocmostClient {
    http: reqwest::Client,
    base: Url,
    token: Option<SecretString>,
}

pub struct DocmostClientBuilder {
    base: Url,
    token: Option<SecretString>,
}

impl DocmostClientBuilder {
    /// Accepts the workspace URL with or without the `/api` suffix and
    /// canonicalises it to `<origin>/api/`.
    pub fn new(api_url: &str) -> Result<Self, DocmostError> {
        let mut base = Url::parse(api_url).map_err(|e| DocmostError::InvalidUrl(e.to_string()))?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(DocmostError::InvalidUrl(
                "URL must be absolute HTTP(S) without credentials, query, or fragment".into(),
            ));
        }
        let path = base.path().trim_end_matches('/');
        let api_path = if path.ends_with("/api") {
            path.to_owned()
        } else {
            format!("{path}/api")
        };
        base.set_path(&(api_path.trim_start_matches('/').to_owned() + "/"));
        Ok(Self { base, token: None })
    }
    pub fn bearer_token(mut self, token: SecretString) -> Self {
        self.token = Some(token);
        self
    }
    pub fn build(self) -> Result<DocmostClient, DocmostError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(transport_error)?;
        Ok(DocmostClient {
            http,
            base: self.base,
            token: self.token,
        })
    }
}

impl DocmostClient {
    pub fn builder(api_url: &str) -> Result<DocmostClientBuilder, DocmostError> {
        DocmostClientBuilder::new(api_url)
    }
    pub fn api_url(&self) -> &Url {
        &self.base
    }
    /// The web application origin, i.e. the API URL without its `/api/`
    /// suffix and without a trailing slash. Docmost serves the client and
    /// the API from the same origin.
    pub fn app_url(&self) -> String {
        let mut url = self.base.clone();
        let path = self.base.path().trim_end_matches('/');
        let path = path.strip_suffix("/api").unwrap_or(path);
        url.set_path(path);
        url.to_string().trim_end_matches('/').to_owned()
    }
    pub fn auth(&self) -> AuthService<'_> {
        AuthService(self)
    }
    pub fn users(&self) -> UserService<'_> {
        UserService(self)
    }
    pub fn workspace(&self) -> WorkspaceService<'_> {
        WorkspaceService(self)
    }
    pub fn spaces(&self) -> SpaceService<'_> {
        SpaceService(self)
    }
    pub fn groups(&self) -> GroupService<'_> {
        GroupService(self)
    }
    pub fn pages(&self) -> PageService<'_> {
        PageService(self)
    }
    pub fn files(&self) -> FileService<'_> {
        FileService(self)
    }
    pub fn comments(&self) -> CommentService<'_> {
        CommentService(self)
    }
    pub fn search(&self) -> SearchService<'_> {
        SearchService(self)
    }

    /// Posts a JSON body to an RPC-style endpoint and unwraps the
    /// `{ success, status, data }` envelope.
    pub async fn post_path<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, DocmostError> {
        let value = self
            .request(Method::POST, path, Some(serde_json::to_value(body)?), true)
            .await?;
        Ok(serde_json::from_value(unwrap_envelope(value))?)
    }
    /// Posts a list request merged with the cursor pagination fields.
    pub async fn list_path<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Value,
        page: &PageRequest,
    ) -> Result<ListResponse<T>, DocmostError> {
        let body = merge(body, serde_json::to_value(page)?);
        let value = self.request(Method::POST, path, Some(body), true).await?;
        Ok(serde_json::from_value(unwrap_envelope(value))?)
    }
    /// Follows `nextCursor` until the server reports no further page.
    pub async fn list_all_path<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Value,
        page: &PageRequest,
    ) -> Result<ListResponse<T>, DocmostError> {
        let mut page = page.clone();
        let mut items = Vec::new();
        loop {
            let response = self.list_path::<T>(path, body.clone(), &page).await?;
            items.extend(response.items);
            let next = response
                .meta
                .as_ref()
                .filter(|m| m.has_next_page == Some(true))
                .and_then(|m| m.next_cursor.clone());
            match next {
                Some(cursor) => page.cursor = Some(cursor),
                None => {
                    return Ok(ListResponse {
                        items,
                        meta: response.meta,
                    });
                }
            }
        }
    }

    fn url(&self, path: &str) -> Result<Url, DocmostError> {
        self.base
            .join(path.trim_start_matches('/'))
            .map_err(|e| DocmostError::InvalidUrl(e.to_string()))
    }
    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, DocmostError> {
        match &self.token {
            Some(token) => Ok(request.bearer_auth(token.expose_secret())),
            None => Err(DocmostError::Unauthorized {
                message: "no bearer token configured".into(),
            }),
        }
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        auth: bool,
    ) -> Result<Value, DocmostError> {
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .header(header::ACCEPT, "application/json");
        if auth {
            request = self.authorize(request)?;
        }
        // Docmost RPC endpoints validate a JSON object; an empty object is
        // the canonical "no arguments" body.
        request = request.json(&body.unwrap_or_else(|| json!({})));
        let response = request.send().await.map_err(transport_error)?;
        let status = response.status();
        let retry_after = retry_after(response.headers());
        let text = response.text().await.map_err(transport_error)?;
        if !status.is_success() {
            return Err(map_error(status, &text, retry_after));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }
    async fn upload(&self, path: &str, form: multipart::Form) -> Result<Value, DocmostError> {
        let request = self
            .http
            .request(Method::POST, self.url(path)?)
            .header(header::ACCEPT, "application/json")
            .timeout(TRANSFER_TIMEOUT)
            .multipart(form);
        let response = self
            .authorize(request)?
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let retry_after = retry_after(response.headers());
        let text = response.text().await.map_err(transport_error)?;
        if !status.is_success() {
            return Err(map_error(status, &text, retry_after));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(unwrap_envelope(serde_json::from_str(&text)?))
    }
    async fn download(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Download, DocmostError> {
        let mut request = self
            .http
            .request(method, self.url(path)?)
            .timeout(TRANSFER_TIMEOUT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self
            .authorize(request)?
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            let text = response.text().await.map_err(transport_error)?;
            return Err(map_error(status, &text, retry_after(&headers)));
        }
        let bytes = response.bytes().await.map_err(transport_error)?.to_vec();
        Ok(Download {
            content_type: header_string(&headers, header::CONTENT_TYPE),
            file_name: header_string(&headers, header::CONTENT_DISPOSITION)
                .as_deref()
                .and_then(disposition_file_name),
            bytes,
        })
    }
}

fn header_string(headers: &header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}
fn retry_after(headers: &header::HeaderMap) -> Option<String> {
    header_string(headers, header::RETRY_AFTER)
}
/// Extracts `filename="..."` (or bare `filename=...`) from a
/// Content-Disposition header, leaving percent-encoding untouched.
fn disposition_file_name(value: &str) -> Option<String> {
    value.split(';').map(str::trim).find_map(|part| {
        let name = part.strip_prefix("filename=")?;
        Some(name.trim_matches('"').to_owned())
    })
}
/// The global response interceptor wraps bodies as `{ success, status, data }`;
/// upload endpoints answer with the raw object instead. Void handlers omit
/// `data`, which becomes `null`.
fn unwrap_envelope(value: Value) -> Value {
    match value {
        Value::Object(mut map) if map.contains_key("success") && map.contains_key("status") => {
            map.remove("data").unwrap_or(Value::Null)
        }
        other => other,
    }
}
fn merge(base: Value, extra: Value) -> Value {
    match (base, extra) {
        (Value::Object(mut base), Value::Object(extra)) => {
            base.extend(extra);
            Value::Object(base)
        }
        (base, _) => base,
    }
}
fn transport_error(error: reqwest::Error) -> DocmostError {
    let category = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "connection, DNS, proxy, or TLS handshake failed"
    } else if error.is_request() {
        "request construction failed"
    } else if error.is_decode() {
        "response body transfer failed"
    } else {
        "HTTP request failed"
    };
    DocmostError::Transport {
        message: format!("{category}: {error:?}"),
    }
}
/// NestJS answers with `{ statusCode, message, error? }` where `message` is a
/// string, or an array of strings for validation failures.
fn error_message(text: &str) -> String {
    let parsed = serde_json::from_str::<Value>(text).ok();
    let message = parsed.as_ref().and_then(|v| match v.get("message") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; "),
        ),
        _ => None,
    });
    message.unwrap_or_else(|| text.to_owned())
}
fn map_error(status: StatusCode, text: &str, retry_after: Option<String>) -> DocmostError {
    let message = error_message(text);
    match status {
        StatusCode::UNAUTHORIZED => DocmostError::Unauthorized { message },
        StatusCode::FORBIDDEN => DocmostError::Forbidden { message },
        StatusCode::NOT_FOUND => DocmostError::NotFound { message },
        StatusCode::PAYLOAD_TOO_LARGE => DocmostError::PayloadTooLarge { message },
        StatusCode::TOO_MANY_REQUESTS => DocmostError::RateLimited {
            message,
            retry_after,
        },
        s if s.is_client_error() => DocmostError::ClientResponse { status: s, message },
        s if s.is_server_error() => DocmostError::ServerResponse { status: s, message },
        s => DocmostError::UnexpectedStatus { status: s, message },
    }
}

pub struct AuthService<'a>(&'a DocmostClient);
impl AuthService<'_> {
    /// Logs in with email and password. Docmost returns the session JWT only
    /// as an `authToken` cookie, which the API also accepts as a bearer token.
    pub async fn login(&self, request: &LoginRequest) -> Result<SecretString, DocmostError> {
        let body = json!({"email": request.email, "password": request.password.expose_secret()});
        let response = self
            .0
            .http
            .request(Method::POST, self.0.url("auth/login")?)
            .header(header::ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let retry_after = retry_after(response.headers());
        let cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(auth_token_cookie);
        let text = response.text().await.map_err(transport_error)?;
        if !status.is_success() {
            return Err(map_error(status, &text, retry_after));
        }
        if let Some(token) = cookie {
            return Ok(SecretString::from(token));
        }
        let requires_mfa = serde_json::from_str::<Value>(&text)
            .ok()
            .map(unwrap_envelope)
            .is_some_and(|v| v.get("userHasMfa").is_some() || v.get("isMfaEnforced").is_some());
        if requires_mfa {
            Err(DocmostError::MfaRequired)
        } else {
            Err(DocmostError::MissingAuthCookie)
        }
    }
    /// Revokes the current session on the server.
    pub async fn logout(&self) -> Result<(), DocmostError> {
        self.0
            .request(Method::POST, "auth/logout", None, true)
            .await
            .map(|_| ())
    }
}
fn auth_token_cookie(value: &str) -> Option<String> {
    let pair = value.split(';').next()?.trim();
    let token = pair.strip_prefix("authToken=")?;
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

pub struct UserService<'a>(&'a DocmostClient);
impl UserService<'_> {
    pub async fn me<T: DeserializeOwned>(&self) -> Result<T, DocmostError> {
        self.0.post_path("users/me", &json!({})).await
    }
}

pub struct WorkspaceService<'a>(&'a DocmostClient);
impl WorkspaceService<'_> {
    pub async fn info<T: DeserializeOwned>(&self) -> Result<T, DocmostError> {
        self.0.post_path("workspace/info", &json!({})).await
    }
    pub async fn members<T: DeserializeOwned>(
        &self,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(self.0, "workspace/members", json!({}), page, all).await
    }
    pub async fn version<T: DeserializeOwned>(&self) -> Result<T, DocmostError> {
        self.0.post_path("version", &json!({})).await
    }
}

pub struct SpaceService<'a>(&'a DocmostClient);
impl SpaceService<'_> {
    pub async fn list<T: DeserializeOwned>(
        &self,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(self.0, "spaces", json!({}), page, all).await
    }
    pub async fn info<T: DeserializeOwned>(&self, space_id: &str) -> Result<T, DocmostError> {
        self.0
            .post_path("spaces/info", &json!({"spaceId": space_id}))
            .await
    }
    pub async fn create<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("spaces/create", body).await
    }
    pub async fn update<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("spaces/update", body).await
    }
    pub async fn delete(&self, space_id: &str) -> Result<(), DocmostError> {
        self.0
            .post_path::<Value, _>("spaces/delete", &json!({"spaceId": space_id}))
            .await
            .map(|_| ())
    }
    pub async fn members<T: DeserializeOwned>(
        &self,
        space_id: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(
            self.0,
            "spaces/members",
            json!({"spaceId": space_id}),
            page,
            all,
        )
        .await
    }
    pub async fn export(
        &self,
        space_id: &str,
        format: ExportFormat,
        include_attachments: bool,
    ) -> Result<Download, DocmostError> {
        self.0
            .download(
                Method::POST,
                "spaces/export",
                Some(json!({
                    "spaceId": space_id,
                    "format": format.as_str(),
                    "includeAttachments": include_attachments,
                })),
            )
            .await
    }
}

pub struct GroupService<'a>(&'a DocmostClient);
impl GroupService<'_> {
    pub async fn list<T: DeserializeOwned>(
        &self,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(self.0, "groups", json!({}), page, all).await
    }
    pub async fn info<T: DeserializeOwned>(&self, group_id: &str) -> Result<T, DocmostError> {
        self.0
            .post_path("groups/info", &json!({"groupId": group_id}))
            .await
    }
    pub async fn members<T: DeserializeOwned>(
        &self,
        group_id: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(
            self.0,
            "groups/members",
            json!({"groupId": group_id}),
            page,
            all,
        )
        .await
    }
}

pub struct PageService<'a>(&'a DocmostClient);
impl PageService<'_> {
    /// `page_id` may be the UUID or the `slugId` from the page URL.
    pub async fn info<T: DeserializeOwned>(
        &self,
        page_id: &str,
        format: Option<ContentFormat>,
        include_space: bool,
    ) -> Result<T, DocmostError> {
        let mut body =
            json!({"pageId": page_id, "includeContent": true, "includeSpace": include_space});
        if let Some(format) = format {
            body["format"] = Value::from(format.as_str());
        }
        self.0.post_path("pages/info", &body).await
    }
    pub async fn create<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("pages/create", body).await
    }
    pub async fn update<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("pages/update", body).await
    }
    pub async fn delete(&self, page_id: &str, permanent: bool) -> Result<(), DocmostError> {
        let mut body = json!({"pageId": page_id});
        if permanent {
            body["permanentlyDelete"] = Value::Bool(true);
        }
        self.0
            .post_path::<Value, _>("pages/delete", &body)
            .await
            .map(|_| ())
    }
    pub async fn restore(&self, page_id: &str) -> Result<(), DocmostError> {
        self.0
            .post_path::<Value, _>("pages/restore", &json!({"pageId": page_id}))
            .await
            .map(|_| ())
    }
    pub async fn recent<T: DeserializeOwned>(
        &self,
        space_id: Option<&str>,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        let mut body = json!({});
        if let Some(space_id) = space_id {
            body["spaceId"] = Value::from(space_id);
        }
        list(self.0, "pages/recent", body, page, all).await
    }
    /// Direct children of `page_id`, or the root pages of `space_id`.
    pub async fn sidebar<T: DeserializeOwned>(
        &self,
        space_id: Option<&str>,
        page_id: Option<&str>,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        let mut body = json!({});
        if let Some(space_id) = space_id {
            body["spaceId"] = Value::from(space_id);
        }
        if let Some(page_id) = page_id {
            body["pageId"] = Value::from(page_id);
        }
        list(self.0, "pages/sidebar-pages", body, page, all).await
    }
    pub async fn trash<T: DeserializeOwned>(
        &self,
        space_id: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(
            self.0,
            "pages/trash",
            json!({"spaceId": space_id}),
            page,
            all,
        )
        .await
    }
    pub async fn move_page(
        &self,
        page_id: &str,
        parent_page_id: Option<&str>,
        position: &str,
    ) -> Result<(), DocmostError> {
        self.0
            .post_path::<Value, _>(
                "pages/move",
                &json!({"pageId": page_id, "parentPageId": parent_page_id, "position": position}),
            )
            .await
            .map(|_| ())
    }
    pub async fn move_to_space<T: DeserializeOwned>(
        &self,
        page_id: &str,
        space_id: &str,
    ) -> Result<T, DocmostError> {
        self.0
            .post_path(
                "pages/move-to-space",
                &json!({"pageId": page_id, "spaceId": space_id}),
            )
            .await
    }
    pub async fn duplicate<T: DeserializeOwned>(
        &self,
        page_id: &str,
        space_id: Option<&str>,
    ) -> Result<T, DocmostError> {
        let mut body = json!({"pageId": page_id});
        if let Some(space_id) = space_id {
            body["spaceId"] = Value::from(space_id);
        }
        self.0.post_path("pages/duplicate", &body).await
    }
    pub async fn history<T: DeserializeOwned>(
        &self,
        page_id: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(
            self.0,
            "pages/history",
            json!({"pageId": page_id}),
            page,
            all,
        )
        .await
    }
    pub async fn history_info<T: DeserializeOwned>(
        &self,
        history_id: &str,
    ) -> Result<T, DocmostError> {
        self.0
            .post_path("pages/history/info", &json!({"historyId": history_id}))
            .await
    }
    pub async fn breadcrumbs<T: DeserializeOwned>(&self, page_id: &str) -> Result<T, DocmostError> {
        self.0
            .post_path("pages/breadcrumbs", &json!({"pageId": page_id}))
            .await
    }
    pub async fn backlinks<T: DeserializeOwned>(
        &self,
        page_id: &str,
        direction: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(
            self.0,
            "pages/backlinks",
            json!({"pageId": page_id, "direction": direction}),
            page,
            all,
        )
        .await
    }
    pub async fn attachments<T: DeserializeOwned>(
        &self,
        page_id: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(
            self.0,
            "pages/attachments",
            json!({"pageId": page_id}),
            page,
            all,
        )
        .await
    }
    pub async fn export(
        &self,
        page_id: &str,
        format: ExportFormat,
        include_children: bool,
        include_attachments: bool,
    ) -> Result<Download, DocmostError> {
        self.0
            .download(
                Method::POST,
                "pages/export",
                Some(json!({
                    "pageId": page_id,
                    "format": format.as_str(),
                    "includeChildren": include_children,
                    "includeAttachments": include_attachments,
                })),
            )
            .await
    }
    /// Imports a `.md`, `.html`, `.docx`, or `.pdf` file as a new root page.
    pub async fn import<T: DeserializeOwned>(
        &self,
        space_id: &str,
        file_name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<T, DocmostError> {
        let form = multipart::Form::new()
            .text("spaceId", space_id.to_owned())
            .part("file", file_part(file_name, mime, bytes)?);
        let value = self.0.upload("pages/import", form).await?;
        Ok(serde_json::from_value(value)?)
    }
}

pub struct FileService<'a>(&'a DocmostClient);
impl FileService<'_> {
    /// Uploads a file attached to `page_id`; the server generates the
    /// attachment id and answers with the attachment record.
    pub async fn upload<T: DeserializeOwned>(
        &self,
        page_id: &str,
        file_name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> Result<T, DocmostError> {
        let form = multipart::Form::new()
            .text("pageId", page_id.to_owned())
            .part("file", file_part(file_name, mime, bytes)?);
        let value = self.0.upload("files/upload", form).await?;
        Ok(serde_json::from_value(value)?)
    }
    pub async fn info<T: DeserializeOwned>(&self, attachment_id: &str) -> Result<T, DocmostError> {
        self.0
            .post_path("files/info", &json!({"attachmentId": attachment_id}))
            .await
    }
    pub async fn download(
        &self,
        attachment_id: &str,
        file_name: &str,
    ) -> Result<Download, DocmostError> {
        self.0
            .download(
                Method::GET,
                &format!("files/{attachment_id}/{file_name}"),
                None,
            )
            .await
    }
}
fn file_part(file_name: &str, mime: &str, bytes: Vec<u8>) -> Result<multipart::Part, DocmostError> {
    multipart::Part::bytes(bytes)
        .file_name(file_name.to_owned())
        .mime_str(mime)
        .map_err(transport_error)
}

pub struct CommentService<'a>(&'a DocmostClient);
impl CommentService<'_> {
    pub async fn list<T: DeserializeOwned>(
        &self,
        page_id: &str,
        page: &PageRequest,
        all: bool,
    ) -> Result<ListResponse<T>, DocmostError> {
        list(self.0, "comments", json!({"pageId": page_id}), page, all).await
    }
    pub async fn info<T: DeserializeOwned>(&self, comment_id: &str) -> Result<T, DocmostError> {
        self.0
            .post_path("comments/info", &json!({"commentId": comment_id}))
            .await
    }
    pub async fn create<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("comments/create", body).await
    }
    pub async fn update<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("comments/update", body).await
    }
    pub async fn delete(&self, comment_id: &str) -> Result<(), DocmostError> {
        self.0
            .post_path::<Value, _>("comments/delete", &json!({"commentId": comment_id}))
            .await
            .map(|_| ())
    }
}

pub struct SearchService<'a>(&'a DocmostClient);
impl SearchService<'_> {
    /// Full-text search; the response is `{ "items": [...] }` without meta.
    pub async fn search<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("search", body).await
    }
    pub async fn suggest<T: DeserializeOwned, B: Serialize>(
        &self,
        body: &B,
    ) -> Result<T, DocmostError> {
        self.0.post_path("search/suggest", body).await
    }
}

async fn list<T: DeserializeOwned>(
    client: &DocmostClient,
    path: &str,
    body: Value,
    page: &PageRequest,
    all: bool,
) -> Result<ListResponse<T>, DocmostError> {
    if all {
        client.list_all_path(path, body, page).await
    } else {
        client.list_path(path, body, page).await
    }
}

/// Page metadata used by the CLI; every other field stays in `extra`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: String,
    pub slug_id: Option<String>,
    pub title: Option<String>,
    pub parent_page_id: Option<String>,
    pub space_id: Option<String>,
    pub position: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Attachment record returned by uploads and `files/info`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub mime_type: Option<String>,
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_auth_token_from_cookie() {
        assert_eq!(
            auth_token_cookie("authToken=abc.def.ghi; Path=/; HttpOnly; SameSite=Lax").as_deref(),
            Some("abc.def.ghi")
        );
        assert_eq!(auth_token_cookie("other=1; Path=/"), None);
        assert_eq!(auth_token_cookie("authToken=; Path=/"), None);
    }

    #[test]
    fn unwraps_envelope_and_keeps_raw_bodies() {
        assert_eq!(
            unwrap_envelope(json!({"success": true, "status": 200, "data": {"id": 1}})),
            json!({"id": 1})
        );
        assert_eq!(
            unwrap_envelope(json!({"success": true, "status": 200})),
            Value::Null
        );
        assert_eq!(
            unwrap_envelope(json!({"id": "raw", "fileName": "a.png"})),
            json!({"id": "raw", "fileName": "a.png"})
        );
    }

    #[test]
    fn joins_validation_messages() {
        assert_eq!(
            error_message(
                r#"{"message":["spaceId must be a UUID","title must be a string"],"error":"Bad Request","statusCode":400}"#
            ),
            "spaceId must be a UUID; title must be a string"
        );
        assert_eq!(
            error_message(r#"{"message":"Page not found","error":"Not Found","statusCode":404}"#),
            "Page not found"
        );
        assert_eq!(error_message("plain text"), "plain text");
    }

    #[test]
    fn parses_content_disposition_file_names() {
        assert_eq!(
            disposition_file_name("attachment; filename=\"My%20Page.md\"").as_deref(),
            Some("My%20Page.md")
        );
        assert_eq!(
            disposition_file_name("attachment; filename=export.zip").as_deref(),
            Some("export.zip")
        );
        assert_eq!(disposition_file_name("inline"), None);
    }

    #[tokio::test]
    async fn transport_error_includes_connection_category() {
        let error = reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .send()
            .await
            .unwrap_err();
        let message = transport_error(error).to_string();
        assert!(message.contains("connection, DNS, proxy, or TLS handshake failed"));
    }
}
