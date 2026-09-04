mod content;
mod links;
mod position;
mod session;

use std::{
    collections::HashMap,
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use directories::ProjectDirs;
use docmost_client::{
    Attachment, ContentFormat, ContentOperation, DocmostClient, DocmostError, Download,
    ExportFormat, LoginRequest, Page, PageRequest,
};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::session::PasswordStore;

#[derive(Parser)]
#[command(name = "docmost-cli", version, about = "Docmost REST API CLI")]
struct Cli {
    #[arg(long, global = true, value_enum, default_value_t = Output::Human)]
    output: Output,
    /// Docmost URL, with or without the /api suffix
    #[arg(long, global = true, env = "DOCMOST_API_URL")]
    api_url: Option<String>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Copy, ValueEnum)]
enum Output {
    Human,
    Json,
}
#[derive(Subcommand)]
enum Command {
    /// Log in, inspect, or end the stored session
    Auth(AuthCommand),
    /// Workspace details and members
    Workspace(WorkspaceCommand),
    /// Spaces, their members, and space exports
    Space(SpaceCommand),
    /// Groups and their members
    Group(GroupCommand),
    /// Pages: content, tree, moves, trash, history, export, import, attachments
    Page(PageCommand),
    /// Uploaded files
    Attachment(AttachmentCommand),
    /// Page and inline comments
    Comment(CommentCommand),
    /// Full-text search over page contents
    Search(SearchArgs),
    /// The current user
    User(UserCommand),
}

#[derive(Args)]
struct AuthCommand {
    #[command(subcommand)]
    action: AuthAction,
}
#[derive(Subcommand)]
enum AuthAction {
    /// Log in with email and password and store the session
    Login(LoginArgs),
    /// Show the stored session, the identity it belongs to, and the server version
    Status,
    /// Revoke the session on the server and remove local credentials
    Logout,
    /// Remove the password saved by `login --remember`, keeping the session
    Forget,
}
#[derive(Args)]
struct LoginArgs {
    #[arg(short, long, env = "DOCMOST_EMAIL")]
    email: Option<String>,
    #[arg(long)]
    password_stdin: bool,
    /// Save the password in the system keychain for silent re-login
    #[arg(long)]
    remember: bool,
}

// Cursor pagination flags shared by every list command.
#[derive(Args, Clone, Default)]
struct ListArgs {
    /// Items per page (1-100, server default 20)
    #[arg(long)]
    limit: Option<u32>,
    /// `meta.nextCursor` from a previous page
    #[arg(long, conflicts_with = "all")]
    cursor: Option<String>,
    /// Filter items by text
    #[arg(long)]
    query: Option<String>,
    /// Follow `nextCursor` until every item is returned
    #[arg(long)]
    all: bool,
}
#[derive(Args)]
struct IdArgs {
    id: String,
}
#[derive(Args)]
struct IdListArgs {
    id: String,
    #[command(flatten)]
    list: ListArgs,
}
#[derive(Args)]
struct DeleteArgs {
    id: String,
    /// Confirm the deletion
    #[arg(long)]
    yes: bool,
}

#[derive(Args)]
struct WorkspaceCommand {
    #[command(subcommand)]
    action: WorkspaceAction,
}
#[derive(Subcommand)]
enum WorkspaceAction {
    /// Workspace details
    Info,
    /// Workspace members
    Members(ListArgs),
}

#[derive(Args)]
struct SpaceCommand {
    #[command(subcommand)]
    action: SpaceAction,
}
#[derive(Subcommand)]
enum SpaceAction {
    /// Spaces visible to the current user
    List(ListArgs),
    /// Space by ID or slug
    Get(IdArgs),
    /// Users and groups with access to a space
    Members(IdListArgs),
    /// Export every page of a space as a zip archive
    Export(SpaceExportArgs),
}
#[derive(Args)]
struct SpaceExportArgs {
    id: String,
    #[arg(long, value_enum, default_value_t = ExportChoice::Markdown)]
    format: ExportChoice,
    #[arg(long)]
    include_attachments: bool,
    /// Destination zip file
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
struct GroupCommand {
    #[command(subcommand)]
    action: GroupAction,
}
#[derive(Subcommand)]
enum GroupAction {
    /// Groups of the workspace
    List(ListArgs),
    /// Group by ID
    Get(IdArgs),
    /// Users in a group
    Members(IdListArgs),
}

#[derive(Args)]
struct PageCommand {
    #[command(subcommand)]
    action: PageAction,
}
#[derive(Subcommand)]
enum PageAction {
    /// Recently updated pages, workspace-wide or within one space
    List(PageListArgs),
    /// Root pages of a space, or the children of one page
    Tree(PageTreeArgs),
    /// Page by ID or slug ID, with its content
    Get(PageGetArgs),
    /// Link to a page in the web app
    Url(IdArgs),
    /// Create a page, optionally with markdown, HTML, or JSON content
    Create(PageCreateArgs),
    /// Change the title, icon, or content of a page
    Edit(PageEditArgs),
    /// Move a page under another parent or to the space root
    Move(PageMoveArgs),
    /// Move a page and its children to another space
    MoveToSpace(PageMoveToSpaceArgs),
    /// Copy a page, optionally into another space
    Duplicate(PageDuplicateArgs),
    /// Move pages to the trash (or delete them permanently)
    Delete(PageDeleteArgs),
    /// Restore a page from the trash
    Restore(IdArgs),
    /// Pages in the trash of a space
    Trash(PageTrashArgs),
    /// Saved versions of a page
    History(IdListArgs),
    /// One history entry by its history ID
    HistoryGet(IdArgs),
    /// Ancestors of a page, from the space root down
    Breadcrumbs(IdArgs),
    /// Pages linking to (or linked from) a page
    Backlinks(PageBacklinksArgs),
    /// Export a page as markdown or HTML (zip when children or attachments are included)
    Export(PageExportArgs),
    /// Create a page from a .md, .html, .docx, or .pdf file
    Import(PageImportArgs),
    /// Files uploaded to a page
    Attachments(IdListArgs),
    /// Upload files and append them to the page content
    Attach(PageAttachArgs),
}
#[derive(Args)]
struct PageListArgs {
    #[arg(long)]
    space: Option<String>,
    #[command(flatten)]
    list: ListArgs,
}
#[derive(Args)]
#[command(group = ArgGroup::new("scope").required(true).multiple(true).args(["space", "parent"]))]
struct PageTreeArgs {
    #[arg(long)]
    space: Option<String>,
    /// Parent page whose children are listed
    #[arg(long)]
    parent: Option<String>,
    /// Walk the whole subtree; items carry a `depth` field
    #[arg(long)]
    recursive: bool,
    #[command(flatten)]
    list: ListArgs,
}
#[derive(Args)]
struct PageGetArgs {
    id: String,
    /// Encoding of the `content` field
    #[arg(long, value_enum, default_value_t = ContentChoice::Markdown)]
    content: ContentChoice,
    #[arg(long)]
    include_space: bool,
}
#[derive(Clone, Copy, ValueEnum)]
enum ContentChoice {
    Markdown,
    Html,
    Json,
    None,
}
#[derive(Clone, Copy, ValueEnum)]
enum FormatChoice {
    Markdown,
    Html,
    Json,
}
impl From<FormatChoice> for ContentFormat {
    fn from(choice: FormatChoice) -> Self {
        match choice {
            FormatChoice::Markdown => ContentFormat::Markdown,
            FormatChoice::Html => ContentFormat::Html,
            FormatChoice::Json => ContentFormat::Json,
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum OperationChoice {
    Replace,
    Append,
    Prepend,
}
impl From<OperationChoice> for ContentOperation {
    fn from(choice: OperationChoice) -> Self {
        match choice {
            OperationChoice::Replace => ContentOperation::Replace,
            OperationChoice::Append => ContentOperation::Append,
            OperationChoice::Prepend => ContentOperation::Prepend,
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum ExportChoice {
    Markdown,
    Html,
}
impl From<ExportChoice> for ExportFormat {
    fn from(choice: ExportChoice) -> Self {
        match choice {
            ExportChoice::Markdown => ExportFormat::Markdown,
            ExportChoice::Html => ExportFormat::Html,
        }
    }
}
// Content input shared by `page create` and `page edit`.
#[derive(Args)]
struct ContentArgs {
    /// Page body as text
    #[arg(long, conflicts_with = "content_file")]
    content: Option<String>,
    /// Read the page body from a file (`-` for stdin)
    #[arg(long)]
    content_file: Option<PathBuf>,
    /// Encoding of the page body
    #[arg(long, value_enum, default_value_t = FormatChoice::Markdown)]
    format: FormatChoice,
    /// Upload local files referenced by markdown links and images, then
    /// rewrite the references to the uploaded files
    #[arg(long)]
    upload_local_files: bool,
    /// Directory local references are resolved against (default: the
    /// content file's directory, or the current directory)
    #[arg(long)]
    attachments_base: Option<PathBuf>,
}
#[derive(Args)]
struct PageCreateArgs {
    #[arg(long)]
    space: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    parent: Option<String>,
    #[arg(long)]
    icon: Option<String>,
    #[command(flatten)]
    content: ContentArgs,
    /// Extra JSON fields merged into the request body
    #[arg(long)]
    data: Option<String>,
}
#[derive(Args)]
struct PageEditArgs {
    id: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    icon: Option<String>,
    #[command(flatten)]
    content: ContentArgs,
    /// How the new content combines with the existing page
    #[arg(long, value_enum, default_value_t = OperationChoice::Replace)]
    operation: OperationChoice,
    /// Extra JSON fields merged into the request body
    #[arg(long)]
    data: Option<String>,
}
#[derive(Args)]
#[command(group = ArgGroup::new("target").required(true).args(["parent", "root"]))]
struct PageMoveArgs {
    id: String,
    /// New parent page
    #[arg(long)]
    parent: Option<String>,
    /// Move to the top level of the space
    #[arg(long)]
    root: bool,
    /// Explicit fractional-index position (5-12 characters)
    #[arg(long, conflicts_with_all = ["first", "after"])]
    position: Option<String>,
    /// Place the page before its new siblings (default: after them)
    #[arg(long, conflicts_with = "after")]
    first: bool,
    /// Place the page right after this sibling
    #[arg(long)]
    after: Option<String>,
}
#[derive(Args)]
struct PageMoveToSpaceArgs {
    id: String,
    #[arg(long)]
    space: String,
}
#[derive(Args)]
struct PageDuplicateArgs {
    id: String,
    /// Duplicate into another space
    #[arg(long)]
    space: Option<String>,
}
#[derive(Args)]
struct PageDeleteArgs {
    #[arg(required = true)]
    ids: Vec<String>,
    /// Confirm the deletion
    #[arg(long)]
    yes: bool,
    /// Skip the trash and delete permanently
    #[arg(long)]
    permanent: bool,
}
#[derive(Args)]
struct PageTrashArgs {
    #[arg(long)]
    space: String,
    #[command(flatten)]
    list: ListArgs,
}
#[derive(Args)]
struct PageBacklinksArgs {
    id: String,
    #[arg(long, value_enum, default_value_t = Direction::Incoming)]
    direction: Direction,
    #[command(flatten)]
    list: ListArgs,
}
#[derive(Clone, Copy, ValueEnum)]
enum Direction {
    Incoming,
    Outgoing,
}
#[derive(Args)]
struct PageExportArgs {
    id: String,
    #[arg(long, value_enum, default_value_t = ExportChoice::Markdown)]
    format: ExportChoice,
    #[arg(long)]
    include_children: bool,
    #[arg(long)]
    include_attachments: bool,
    /// Destination file (defaults to stdout for single-page text exports)
    #[arg(long)]
    out: Option<PathBuf>,
}
#[derive(Args)]
struct PageImportArgs {
    #[arg(long)]
    space: String,
    #[arg(long)]
    file: PathBuf,
    /// Move the imported page under this parent
    #[arg(long)]
    parent: Option<String>,
}
#[derive(Args)]
struct PageAttachArgs {
    id: String,
    #[arg(long, required = true)]
    file: Vec<PathBuf>,
    /// Upload only, without appending nodes to the page content
    #[arg(long)]
    no_insert: bool,
}

#[derive(Args)]
struct AttachmentCommand {
    #[command(subcommand)]
    action: AttachmentAction,
}
#[derive(Subcommand)]
enum AttachmentAction {
    /// Attachment metadata by ID
    Info(IdArgs),
    /// Download an attachment by ID and file name
    Download(AttachmentDownloadArgs),
}
#[derive(Args)]
struct AttachmentDownloadArgs {
    id: String,
    file_name: String,
    /// Destination file (defaults to stdout)
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct CommentCommand {
    #[command(subcommand)]
    action: CommentAction,
}
#[derive(Subcommand)]
enum CommentAction {
    /// Comments on a page
    List(CommentListArgs),
    /// Comment by ID
    Get(IdArgs),
    /// Add a page comment, an inline comment, or a reply
    Create(CommentCreateArgs),
    /// Replace the body of a comment
    Edit(CommentEditArgs),
    /// Delete a comment
    Delete(DeleteArgs),
}
#[derive(Args)]
struct CommentListArgs {
    #[arg(long)]
    page: String,
    #[command(flatten)]
    list: ListArgs,
}
#[derive(Args)]
#[command(group = ArgGroup::new("body").required(true).args(["text", "content_json"]))]
struct CommentCreateArgs {
    #[arg(long)]
    page: String,
    /// Plain-text comment body
    #[arg(long)]
    text: Option<String>,
    /// ProseMirror document JSON for the comment body
    #[arg(long)]
    content_json: Option<String>,
    /// Exact page text the comment refers to (creates an inline comment)
    #[arg(long)]
    selection: Option<String>,
    /// Reply to this comment
    #[arg(long)]
    parent: Option<String>,
}
#[derive(Args)]
#[command(group = ArgGroup::new("body").required(true).args(["text", "content_json"]))]
struct CommentEditArgs {
    id: String,
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    content_json: Option<String>,
}

#[derive(Args)]
struct SearchArgs {
    query: String,
    #[arg(long)]
    space: Option<String>,
    /// Match titles only
    #[arg(long)]
    title_only: bool,
    #[arg(long)]
    limit: Option<u32>,
    #[arg(long)]
    offset: Option<u32>,
    /// Restrict to pages created by this user ID
    #[arg(long)]
    creator: Option<String>,
    /// Restrict to pages carrying this label ID (repeatable)
    #[arg(long)]
    label: Vec<String>,
}

#[derive(Args)]
struct UserCommand {
    #[command(subcommand)]
    action: UserAction,
}
#[derive(Subcommand)]
enum UserAction {
    /// The authenticated user and workspace
    Me,
}

#[derive(Debug, Error)]
enum AppError {
    #[error("{0}")]
    Client(#[from] DocmostError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Failed(String),
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    api_url: Option<String>,
    auth_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    remember_password: bool,
}

fn config_path() -> Result<PathBuf, AppError> {
    if let Some(path) = std::env::var_os("DOCMOST_CONFIG") {
        return Ok(path.into());
    }
    ProjectDirs::from("", "", "docmost-cli")
        .map(|d| d.config_dir().join("config.json"))
        .ok_or_else(|| AppError::Config("unable to determine configuration directory".into()))
}
fn load_config(path: &PathBuf) -> Result<Config, AppError> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e.into()),
    }
}
fn save_config(path: &PathBuf, config: &Config) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    let data = serde_json::to_vec_pretty(config)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(tmp, path)?;
    Ok(())
}
fn api_url(cli_url: &Option<String>, config: &Config) -> Result<String, AppError> {
    cli_url
        .clone()
        .or_else(|| std::env::var("DOCMOST_API_URL").ok())
        .or_else(|| config.api_url.clone())
        .ok_or_else(|| {
            AppError::Config(
                "Docmost URL required via --api-url, DOCMOST_API_URL, or `docmost-cli auth login`"
                    .into(),
            )
        })
}
fn client(url: &str, access_token: Option<&str>) -> Result<DocmostClient, AppError> {
    let builder = DocmostClient::builder(url)?;
    let builder = match access_token {
        Some(token) => builder.bearer_token(SecretString::from(token.to_owned())),
        None => builder,
    };
    Ok(builder.build()?)
}
fn page_request(args: &ListArgs) -> Result<PageRequest, AppError> {
    if let Some(limit) = args.limit
        && !(1..=100).contains(&limit)
    {
        return Err(AppError::Usage("--limit must be between 1 and 100".into()));
    }
    Ok(PageRequest {
        limit: args.limit,
        cursor: args.cursor.clone(),
        before_cursor: None,
        query: args.query.clone(),
    })
}
/// Parses `--data` into the object that explicit flags are merged into.
fn data_object(data: &Option<String>) -> Result<Map<String, Value>, AppError> {
    match data {
        Some(data) => serde_json::from_str::<Value>(data)?
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::Usage("--data must be a JSON object".into())),
        None => Ok(Map::new()),
    }
}
fn emit(output: Output, value: &impl Serialize) -> Result<(), AppError> {
    match output {
        Output::Json => println!("{}", serde_json::to_string_pretty(value)?),
        Output::Human => println!("{}", serde_json::to_string_pretty(value)?),
    }
    Ok(())
}
fn mime_for(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_owned()
}
fn file_name_of(path: &Path) -> Result<String, AppError> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| AppError::Usage(format!("{} has no file name", path.display())))
}
/// Writes a download to `--out`, or streams text to stdout when no file is
/// given. Binary bodies (zip archives) always need `--out`.
fn deliver(output: Output, download: &Download, out: &Option<PathBuf>) -> Result<(), AppError> {
    let file_name = download.file_name.as_deref().map(|name| {
        percent_encoding::percent_decode_str(name)
            .decode_utf8_lossy()
            .into_owned()
    });
    match out {
        Some(path) => {
            fs::write(path, &download.bytes)?;
            emit(
                output,
                &json!({
                    "written": path,
                    "bytes": download.bytes.len(),
                    "file_name": file_name,
                    "content_type": download.content_type,
                }),
            )
        }
        None => {
            // Exports arrive as application/octet-stream, so the file name
            // decides whether the body is safe to print.
            let textual_type = download
                .content_type
                .as_deref()
                .is_some_and(|t| t.starts_with("text/") || t.contains("json"));
            let textual_name = file_name.as_deref().is_some_and(|name| {
                let lower = name.to_ascii_lowercase();
                [".md", ".markdown", ".html", ".htm", ".txt", ".json"]
                    .iter()
                    .any(|ext| lower.ends_with(ext))
            });
            if !textual_type && !textual_name {
                return Err(AppError::Usage(
                    "the server sent a binary file; pass --out <FILE> to save it".into(),
                ));
            }
            let mut stdout = io::stdout().lock();
            stdout.write_all(&download.bytes)?;
            stdout.flush()?;
            Ok(())
        }
    }
}

/// Text supplied through `--content` or `--content-file`, plus the directory
/// that relative attachment paths are resolved against.
struct ContentInput {
    text: String,
    base: PathBuf,
}
fn read_content(args: &ContentArgs) -> Result<Option<ContentInput>, AppError> {
    let cwd = std::env::current_dir()?;
    let explicit_base = args.attachments_base.clone();
    if let Some(text) = &args.content {
        return Ok(Some(ContentInput {
            text: text.clone(),
            base: explicit_base.unwrap_or(cwd),
        }));
    }
    let Some(file) = &args.content_file else {
        return Ok(None);
    };
    if file.as_os_str() == "-" {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        return Ok(Some(ContentInput {
            text,
            base: explicit_base.unwrap_or(cwd),
        }));
    }
    let text = fs::read_to_string(file)?;
    let base = explicit_base
        .or_else(|| file.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(cwd);
    Ok(Some(ContentInput { text, base }))
}
/// Applies the markdown shorthands and encodes the body for the request.
fn content_value(text: &str, format: ContentFormat) -> Result<Value, AppError> {
    Ok(match format {
        ContentFormat::Markdown => Value::String(content::preprocess_status_tags(text)),
        ContentFormat::Html => Value::String(text.to_owned()),
        ContentFormat::Json => serde_json::from_str(text)?,
    })
}

/// Caches space slugs so lists resolve each space at most once.
#[derive(Default)]
struct SpaceSlugs {
    cache: HashMap<String, Option<String>>,
}
impl SpaceSlugs {
    async fn slug(&mut self, client: &DocmostClient, space_id: &str) -> Option<String> {
        if let Some(slug) = self.cache.get(space_id) {
            return slug.clone();
        }
        let slug = client
            .spaces()
            .info::<Value>(space_id)
            .await
            .ok()
            .and_then(|space| space["slug"].as_str().map(str::to_owned));
        self.cache.insert(space_id.to_owned(), slug.clone());
        slug
    }
}

/// Adds a `url` field to every page-like object (anything carrying a
/// `slugId`) inside `value`, so callers never have to build links by hand.
/// The space slug comes from an embedded `space` object or is looked up by
/// `spaceId`; page bodies are not traversed.
async fn add_urls(client: &DocmostClient, slugs: &mut SpaceSlugs, value: &mut Value) {
    let app_url = client.app_url();
    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        match current {
            Value::Array(items) => stack.extend(items.iter_mut()),
            Value::Object(object) => {
                let slug_id = object
                    .get("slugId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(slug_id) = slug_id {
                    let embedded = object
                        .get("space")
                        .and_then(|space| space.get("slug"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let space_slug = match embedded {
                        Some(slug) => Some(slug),
                        None => {
                            let space_id = object
                                .get("spaceId")
                                .and_then(Value::as_str)
                                .or_else(|| {
                                    object
                                        .get("space")
                                        .and_then(|space| space.get("id"))
                                        .and_then(Value::as_str)
                                })
                                .map(str::to_owned);
                            match space_id {
                                Some(id) => slugs.slug(client, &id).await,
                                None => None,
                            }
                        }
                    };
                    if let Some(space_slug) = space_slug {
                        let title = object.get("title").and_then(Value::as_str);
                        let url = links::page_url(&app_url, &space_slug, &slug_id, title);
                        object.insert("url".into(), Value::String(url));
                    }
                }
                for (key, child) in object.iter_mut() {
                    if !matches!(key.as_str(), "content" | "ydoc" | "textContent")
                        && (child.is_array() || child.is_object())
                    {
                        stack.push(child);
                    }
                }
            }
            _ => {}
        }
    }
}

async fn emit_pages(
    client: &DocmostClient,
    slugs: &mut SpaceSlugs,
    output: Output,
    mut value: Value,
) -> Result<(), AppError> {
    add_urls(client, slugs, &mut value).await;
    emit(output, &value)
}

async fn upload_file(
    client: &DocmostClient,
    page_id: &str,
    path: &Path,
) -> Result<Attachment, AppError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| AppError::Usage(format!("unable to read {}: {e}", path.display())))?;
    Ok(client
        .files()
        .upload(page_id, &file_name_of(path)?, &mime_for(path), bytes)
        .await?)
}

/// Uploads every local file referenced by the markdown and rewrites the
/// references to the uploaded URLs. Returns the new text and the uploads.
async fn upload_local_references(
    client: &DocmostClient,
    page_id: &str,
    input: &ContentInput,
) -> Result<(String, Vec<Attachment>), AppError> {
    let references = content::local_references(&input.text);
    let mut urls = Vec::with_capacity(references.len());
    let mut uploads = Vec::with_capacity(references.len());
    for reference in &references {
        let path = content::resolve_local(&input.base, &reference.target);
        let attachment = upload_file(client, page_id, &path).await?;
        urls.push(content::file_url(&attachment));
        uploads.push(attachment);
    }
    Ok((
        content::replace_targets(&input.text, &references, &urls),
        uploads,
    ))
}

fn with_uploads(mut page: Value, uploads: Vec<Attachment>) -> Value {
    if !uploads.is_empty()
        && let Some(object) = page.as_object_mut()
    {
        object.insert("uploads".into(), json!(uploads));
    }
    page
}

async fn create_page(
    client: &DocmostClient,
    slugs: &mut SpaceSlugs,
    args: &PageCreateArgs,
    output: Output,
) -> Result<(), AppError> {
    let format = ContentFormat::from(args.content.format);
    let input = read_content(&args.content)?;
    if args.content.upload_local_files && format != ContentFormat::Markdown {
        return Err(AppError::Usage(
            "--upload-local-files requires --format markdown".into(),
        ));
    }
    let mut body = data_object(&args.data)?;
    body.insert("spaceId".into(), json!(args.space));
    if let Some(title) = &args.title {
        body.insert("title".into(), json!(title));
    }
    if let Some(parent) = &args.parent {
        body.insert("parentPageId".into(), json!(parent));
    }
    if let Some(icon) = &args.icon {
        body.insert("icon".into(), json!(icon));
    }
    let deferred = input.as_ref().filter(|input| {
        args.content.upload_local_files && !content::local_references(&input.text).is_empty()
    });
    if let Some(input) = &input
        && deferred.is_none()
    {
        body.insert("content".into(), content_value(&input.text, format)?);
        body.insert("format".into(), json!(format.as_str()));
    }
    let created: Value = client.pages().create(&Value::Object(body)).await?;
    let Some(input) = deferred else {
        return emit_pages(client, slugs, output, created).await;
    };
    // Attachments need the page ID, so the page is created first and the
    // content is written once the uploaded URLs are known.
    let page_id = created["id"]
        .as_str()
        .ok_or_else(|| AppError::Failed("pages/create returned no page id".into()))?
        .to_owned();
    let (text, uploads) = upload_local_references(client, &page_id, input).await?;
    let updated: Value = client
        .pages()
        .update(&json!({
            "pageId": page_id,
            "content": content_value(&text, format)?,
            "operation": ContentOperation::Replace.as_str(),
            "format": format.as_str(),
        }))
        .await?;
    emit_pages(client, slugs, output, with_uploads(updated, uploads)).await
}

async fn edit_page(
    client: &DocmostClient,
    slugs: &mut SpaceSlugs,
    args: &PageEditArgs,
    output: Output,
) -> Result<(), AppError> {
    let format = ContentFormat::from(args.content.format);
    let input = read_content(&args.content)?;
    if args.content.upload_local_files && format != ContentFormat::Markdown {
        return Err(AppError::Usage(
            "--upload-local-files requires --format markdown".into(),
        ));
    }
    let mut body = data_object(&args.data)?;
    if let Some(title) = &args.title {
        body.insert("title".into(), json!(title));
    }
    if let Some(icon) = &args.icon {
        body.insert("icon".into(), json!(icon));
    }
    let mut uploads = Vec::new();
    if let Some(input) = &input {
        let text = if args.content.upload_local_files {
            let (text, uploaded) = upload_local_references(client, &args.id, input).await?;
            uploads = uploaded;
            text
        } else {
            input.text.clone()
        };
        body.insert("content".into(), content_value(&text, format)?);
        body.insert(
            "operation".into(),
            json!(ContentOperation::from(args.operation).as_str()),
        );
        body.insert("format".into(), json!(format.as_str()));
    }
    if body.is_empty() {
        return Err(AppError::Usage("edit needs at least one field".into()));
    }
    body.insert("pageId".into(), json!(args.id));
    let updated: Value = client.pages().update(&Value::Object(body)).await?;
    emit_pages(client, slugs, output, with_uploads(updated, uploads)).await
}

/// Siblings under `parent` (or the space root), sorted by position and
/// excluding `moving` itself.
async fn siblings(
    client: &DocmostClient,
    space_id: &str,
    parent: Option<&str>,
    moving: &str,
) -> Result<Vec<Page>, AppError> {
    let mut pages: Vec<Page> = client
        .pages()
        .sidebar(Some(space_id), parent, &PageRequest::default(), true)
        .await?
        .items;
    pages.retain(|page| page.id != moving);
    pages.sort_by(|a, b| a.position.cmp(&b.position));
    Ok(pages)
}
fn bound(page: Option<&Page>) -> Result<Option<String>, AppError> {
    page.and_then(|page| page.position.as_deref())
        .map(|position| position::normalize_position(position).map_err(AppError::Failed))
        .transpose()
}

async fn move_page(
    client: &DocmostClient,
    args: &PageMoveArgs,
    output: Output,
) -> Result<(), AppError> {
    let parent = args.parent.as_deref();
    let position = match &args.position {
        Some(position) => position.clone(),
        None => {
            let page: Page = client.pages().info(&args.id, None, false).await?;
            let space_id = page
                .space_id
                .clone()
                .ok_or_else(|| AppError::Failed("page has no spaceId".into()))?;
            let siblings = siblings(client, &space_id, parent, &page.id).await?;
            let (before, after) = if args.first {
                (None, siblings.first())
            } else if let Some(anchor) = &args.after {
                let index = siblings
                    .iter()
                    .position(|s| &s.id == anchor || s.slug_id.as_deref() == Some(anchor))
                    .ok_or_else(|| {
                        AppError::Usage(format!("{anchor} is not a child of the target parent"))
                    })?;
                (siblings.get(index), siblings.get(index + 1))
            } else {
                (siblings.last(), None)
            };
            position::position_between(bound(before)?.as_deref(), bound(after)?.as_deref())
                .map_err(AppError::Failed)?
        }
    };
    client
        .pages()
        .move_page(&args.id, parent, &position)
        .await?;
    emit(
        output,
        &json!({"moved": true, "id": args.id, "parentPageId": parent, "position": position}),
    )
}

async fn page_tree(
    client: &DocmostClient,
    slugs: &mut SpaceSlugs,
    args: &PageTreeArgs,
    output: Output,
) -> Result<(), AppError> {
    let page = page_request(&args.list)?;
    let space = args.space.as_deref();
    let parent = args.parent.as_deref();
    let first = client
        .pages()
        .sidebar::<Value>(space, parent, &page, args.list.all || args.recursive)
        .await?;
    if !args.recursive {
        return emit_pages(client, slugs, output, serde_json::to_value(first)?).await;
    }
    let mut items = Vec::new();
    let mut queue: Vec<(Value, u32)> = first.items.into_iter().map(|item| (item, 0)).collect();
    queue.reverse();
    while let Some((mut item, depth)) = queue.pop() {
        let id = item["id"].as_str().map(str::to_owned);
        let has_children = item["hasChildren"].as_bool().unwrap_or(true);
        if let Some(object) = item.as_object_mut() {
            object.insert("depth".into(), json!(depth));
        }
        items.push(item);
        if let Some(id) = id
            && has_children
        {
            let children = client
                .pages()
                .sidebar::<Value>(space, Some(&id), &PageRequest::default(), true)
                .await?;
            for child in children.items.into_iter().rev() {
                queue.push((child, depth + 1));
            }
        }
    }
    emit_pages(client, slugs, output, json!({"items": items})).await
}

async fn import_page(
    client: &DocmostClient,
    slugs: &mut SpaceSlugs,
    args: &PageImportArgs,
    output: Output,
) -> Result<(), AppError> {
    let bytes = tokio::fs::read(&args.file)
        .await
        .map_err(|e| AppError::Usage(format!("unable to read {}: {e}", args.file.display())))?;
    let mut page: Value = client
        .pages()
        .import(
            &args.space,
            &file_name_of(&args.file)?,
            &mime_for(&args.file),
            bytes,
        )
        .await?;
    if let Some(object) = page.as_object_mut() {
        // The import handler answers with the whole row, including the
        // binary Yjs state; keep the output to metadata.
        for key in ["content", "textContent", "ydoc"] {
            object.remove(key);
        }
    }
    if let Some(parent) = &args.parent {
        let page_id = page["id"]
            .as_str()
            .ok_or_else(|| AppError::Failed("pages/import returned no page id".into()))?
            .to_owned();
        let siblings = siblings(client, &args.space, Some(parent), &page_id).await?;
        let position = position::position_between(bound(siblings.last())?.as_deref(), None)
            .map_err(AppError::Failed)?;
        client
            .pages()
            .move_page(&page_id, Some(parent), &position)
            .await?;
        if let Some(object) = page.as_object_mut() {
            object.insert("parentPageId".into(), json!(parent));
            object.insert("position".into(), json!(position));
        }
    }
    emit_pages(client, slugs, output, page).await
}

async fn attach(
    client: &DocmostClient,
    args: &PageAttachArgs,
    output: Output,
) -> Result<(), AppError> {
    let mut attachments = Vec::with_capacity(args.file.len());
    for path in &args.file {
        attachments.push(upload_file(client, &args.id, path).await?);
    }
    if args.no_insert {
        return emit(
            output,
            &json!({"attachments": attachments, "inserted": false}),
        );
    }
    let nodes = attachments.iter().map(content::attachment_node).collect();
    client
        .pages()
        .update::<Value, _>(&json!({
            "pageId": args.id,
            "content": content::document(nodes),
            "operation": ContentOperation::Append.as_str(),
            "format": ContentFormat::Json.as_str(),
        }))
        .await?;
    emit(
        output,
        &json!({"attachments": attachments, "inserted": true}),
    )
}

async fn delete_pages(
    client: &DocmostClient,
    args: &PageDeleteArgs,
    output: Output,
) -> Result<(), AppError> {
    let mut results = Vec::with_capacity(args.ids.len());
    let mut first_error = None;
    for id in &args.ids {
        match client.pages().delete(id, args.permanent).await {
            Ok(()) => {
                results.push(json!({"id": id, "deleted": true, "permanent": args.permanent}));
            }
            Err(error) => {
                results.push(json!({"id": id, "deleted": false, "error": error.to_string()}));
                first_error.get_or_insert(error);
            }
        }
    }
    if args.ids.len() == 1 {
        return match first_error {
            Some(error) => Err(error.into()),
            None => emit(output, &results[0]),
        };
    }
    let failures = results.iter().filter(|r| r["deleted"] == false).count();
    emit(output, &json!({"results": results}))?;
    if failures > 0 {
        return Err(AppError::Failed(format!(
            "{failures} of {} deletions failed",
            args.ids.len()
        )));
    }
    Ok(())
}

fn comment_content(
    text: &Option<String>,
    content_json: &Option<String>,
) -> Result<String, AppError> {
    match (text, content_json) {
        (Some(text), _) => Ok(content::comment_document(text)),
        (None, Some(raw)) => {
            // Validate early: the server expects a JSON-encoded string.
            let value: Value = serde_json::from_str(raw)?;
            Ok(value.to_string())
        }
        (None, None) => Err(AppError::Usage(
            "--text or --content-json is required".into(),
        )),
    }
}

async fn execute_authenticated(
    client: &DocmostClient,
    config: &Config,
    command: &Command,
    output: Output,
) -> Result<(), AppError> {
    let mut slugs = SpaceSlugs::default();
    match command {
        Command::Auth(AuthCommand {
            action: AuthAction::Status,
        }) => {
            let identity: Value = client.users().me().await?;
            let server_version = client
                .workspace()
                .version::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("currentVersion").cloned())
                .unwrap_or(Value::Null);
            emit(
                output,
                &json!({
                    "api_url": client.api_url().as_str(),
                    "email": config.email,
                    "identity": identity,
                    "server_version": server_version,
                    "password_stored": config.remember_password,
                }),
            )
        }
        Command::Auth(_) => unreachable!(),
        Command::Workspace(workspace) => match &workspace.action {
            WorkspaceAction::Info => emit(output, &client.workspace().info::<Value>().await?),
            WorkspaceAction::Members(list) => emit(
                output,
                &client
                    .workspace()
                    .members::<Value>(&page_request(list)?, list.all)
                    .await?,
            ),
        },
        Command::Space(space) => match &space.action {
            SpaceAction::List(list) => emit(
                output,
                &client
                    .spaces()
                    .list::<Value>(&page_request(list)?, list.all)
                    .await?,
            ),
            SpaceAction::Get(args) => emit(output, &client.spaces().info::<Value>(&args.id).await?),
            SpaceAction::Members(args) => emit(
                output,
                &client
                    .spaces()
                    .members::<Value>(&args.id, &page_request(&args.list)?, args.list.all)
                    .await?,
            ),
            SpaceAction::Export(args) => {
                let download = client
                    .spaces()
                    .export(&args.id, args.format.into(), args.include_attachments)
                    .await?;
                deliver(output, &download, &Some(args.out.clone()))
            }
        },
        Command::Group(group) => match &group.action {
            GroupAction::List(list) => emit(
                output,
                &client
                    .groups()
                    .list::<Value>(&page_request(list)?, list.all)
                    .await?,
            ),
            GroupAction::Get(args) => emit(output, &client.groups().info::<Value>(&args.id).await?),
            GroupAction::Members(args) => emit(
                output,
                &client
                    .groups()
                    .members::<Value>(&args.id, &page_request(&args.list)?, args.list.all)
                    .await?,
            ),
        },
        Command::Page(page) => match &page.action {
            PageAction::List(args) => {
                let pages = client
                    .pages()
                    .recent::<Value>(
                        args.space.as_deref(),
                        &page_request(&args.list)?,
                        args.list.all,
                    )
                    .await?;
                emit_pages(client, &mut slugs, output, serde_json::to_value(pages)?).await
            }
            PageAction::Tree(args) => page_tree(client, &mut slugs, args, output).await,
            PageAction::Get(args) => {
                let format = match args.content {
                    ContentChoice::Markdown => Some(ContentFormat::Markdown),
                    ContentChoice::Html => Some(ContentFormat::Html),
                    ContentChoice::Json | ContentChoice::None => None,
                };
                // The space is always fetched so the link can be built;
                // it stays in the output only when asked for.
                let mut page: Value = client.pages().info(&args.id, format, true).await?;
                add_urls(client, &mut slugs, &mut page).await;
                if let Some(object) = page.as_object_mut() {
                    if !args.include_space {
                        object.remove("space");
                    }
                    if matches!(args.content, ContentChoice::None) {
                        object.remove("content");
                    }
                }
                emit(output, &page)
            }
            PageAction::Url(args) => {
                let mut page: Value = client.pages().info(&args.id, None, true).await?;
                add_urls(client, &mut slugs, &mut page).await;
                let url = page["url"].as_str().ok_or_else(|| {
                    AppError::Failed("unable to build the page link: no space slug".into())
                })?;
                emit(
                    output,
                    &json!({
                        "id": page["id"],
                        "slugId": page["slugId"],
                        "title": page["title"],
                        "spaceSlug": page["space"]["slug"],
                        "url": url,
                    }),
                )
            }
            PageAction::Create(args) => create_page(client, &mut slugs, args, output).await,
            PageAction::Edit(args) => edit_page(client, &mut slugs, args, output).await,
            PageAction::Move(args) => move_page(client, args, output).await,
            PageAction::MoveToSpace(args) => emit(
                output,
                &client
                    .pages()
                    .move_to_space::<Value>(&args.id, &args.space)
                    .await?,
            ),
            PageAction::Duplicate(args) => {
                let page = client
                    .pages()
                    .duplicate::<Value>(&args.id, args.space.as_deref())
                    .await?;
                emit_pages(client, &mut slugs, output, page).await
            }
            PageAction::Delete(args) => delete_pages(client, args, output).await,
            PageAction::Restore(args) => {
                client.pages().restore(&args.id).await?;
                emit(output, &json!({"restored": true, "id": args.id}))
            }
            PageAction::Trash(args) => {
                let pages = client
                    .pages()
                    .trash::<Value>(&args.space, &page_request(&args.list)?, args.list.all)
                    .await?;
                emit_pages(client, &mut slugs, output, serde_json::to_value(pages)?).await
            }
            PageAction::History(args) => emit(
                output,
                &client
                    .pages()
                    .history::<Value>(&args.id, &page_request(&args.list)?, args.list.all)
                    .await?,
            ),
            PageAction::HistoryGet(args) => emit(
                output,
                &client.pages().history_info::<Value>(&args.id).await?,
            ),
            PageAction::Breadcrumbs(args) => {
                let crumbs = client.pages().breadcrumbs::<Value>(&args.id).await?;
                emit_pages(client, &mut slugs, output, crumbs).await
            }
            PageAction::Backlinks(args) => {
                let direction = match args.direction {
                    Direction::Incoming => "incoming",
                    Direction::Outgoing => "outgoing",
                };
                let pages = client
                    .pages()
                    .backlinks::<Value>(
                        &args.id,
                        direction,
                        &page_request(&args.list)?,
                        args.list.all,
                    )
                    .await?;
                emit_pages(client, &mut slugs, output, serde_json::to_value(pages)?).await
            }
            PageAction::Export(args) => {
                let download = client
                    .pages()
                    .export(
                        &args.id,
                        args.format.into(),
                        args.include_children,
                        args.include_attachments,
                    )
                    .await?;
                deliver(output, &download, &args.out)
            }
            PageAction::Import(args) => import_page(client, &mut slugs, args, output).await,
            PageAction::Attachments(args) => emit(
                output,
                &client
                    .pages()
                    .attachments::<Value>(&args.id, &page_request(&args.list)?, args.list.all)
                    .await?,
            ),
            PageAction::Attach(args) => attach(client, args, output).await,
        },
        Command::Attachment(attachment) => match &attachment.action {
            AttachmentAction::Info(args) => {
                emit(output, &client.files().info::<Value>(&args.id).await?)
            }
            AttachmentAction::Download(args) => {
                let download = client.files().download(&args.id, &args.file_name).await?;
                match &args.out {
                    Some(_) => deliver(output, &download, &args.out),
                    None => {
                        let mut stdout = io::stdout().lock();
                        stdout.write_all(&download.bytes)?;
                        stdout.flush()?;
                        Ok(())
                    }
                }
            }
        },
        Command::Comment(comment) => match &comment.action {
            CommentAction::List(args) => emit(
                output,
                &client
                    .comments()
                    .list::<Value>(&args.page, &page_request(&args.list)?, args.list.all)
                    .await?,
            ),
            CommentAction::Get(args) => {
                emit(output, &client.comments().info::<Value>(&args.id).await?)
            }
            CommentAction::Create(args) => {
                let mut body = json!({
                    "pageId": args.page,
                    "content": comment_content(&args.text, &args.content_json)?,
                });
                if let Some(selection) = &args.selection {
                    body["selection"] = json!(selection);
                    body["type"] = json!("inline");
                }
                if let Some(parent) = &args.parent {
                    body["parentCommentId"] = json!(parent);
                }
                emit(output, &client.comments().create::<Value, _>(&body).await?)
            }
            CommentAction::Edit(args) => emit(
                output,
                &client
                    .comments()
                    .update::<Value, _>(&json!({
                        "commentId": args.id,
                        "content": comment_content(&args.text, &args.content_json)?,
                    }))
                    .await?,
            ),
            CommentAction::Delete(args) => {
                client.comments().delete(&args.id).await?;
                emit(output, &json!({"deleted": true, "id": args.id}))
            }
        },
        Command::Search(args) => {
            let mut body = json!({"query": args.query});
            if let Some(space) = &args.space {
                body["spaceId"] = json!(space);
            }
            if args.title_only {
                body["titleOnly"] = json!(true);
            }
            if let Some(limit) = args.limit {
                body["limit"] = json!(limit);
            }
            if let Some(offset) = args.offset {
                body["offset"] = json!(offset);
            }
            if let Some(creator) = &args.creator {
                body["creatorId"] = json!(creator);
            }
            if !args.label.is_empty() {
                body["labelIds"] = json!(args.label);
            }
            let results = client.search().search::<Value, _>(&body).await?;
            emit_pages(client, &mut slugs, output, results).await
        }
        Command::User(user) => match &user.action {
            UserAction::Me => emit(output, &client.users().me::<Value>().await?),
        },
    }
}

/// Rejects destructive commands before any network access.
fn validate(command: &Command) -> Result<(), AppError> {
    match command {
        Command::Page(PageCommand {
            action: PageAction::Delete(args),
        }) if !args.yes => Err(AppError::Usage("delete requires --yes".into())),
        Command::Comment(CommentCommand {
            action: CommentAction::Delete(args),
        }) if !args.yes => Err(AppError::Usage("delete requires --yes".into())),
        _ => Ok(()),
    }
}

/// Credentials usable for a silent re-login once the session token is gone.
struct ReloginCredentials {
    email: String,
    password: SecretString,
    /// Whether the resulting token belongs in the config file.
    persist: bool,
}

fn session_expired() -> AppError {
    AppError::Client(DocmostError::Unauthorized {
        message: "no valid session, run `docmost-cli auth login`".into(),
    })
}

async fn password_from_store(
    store: &PasswordStore,
    url: &str,
    email: &str,
) -> Option<SecretString> {
    let store = store.clone();
    let (url, email) = (url.to_owned(), email.to_owned());
    let lookup = tokio::task::spawn_blocking(move || store.get(&url, &email)).await;
    match lookup {
        Ok(Ok(password)) => password,
        Ok(Err(error)) => {
            eprintln!("warning: {error}");
            None
        }
        Err(error) => {
            eprintln!("warning: password lookup failed: {error}");
            None
        }
    }
}

/// Resolves credentials for re-login in priority order: `DOCMOST_PASSWORD`
/// from the environment, the password saved with `login --remember`, and
/// finally an interactive prompt when stdin is a terminal.
async fn relogin_credentials(
    config: &Config,
    url: &str,
    access_from_environment: bool,
    allow_stored: bool,
) -> Option<ReloginCredentials> {
    let email = std::env::var("DOCMOST_EMAIL")
        .ok()
        .or_else(|| config.email.clone())?;
    if let Ok(password) = std::env::var("DOCMOST_PASSWORD") {
        return Some(ReloginCredentials {
            email,
            password: SecretString::from(password),
            persist: false,
        });
    }
    if access_from_environment || !allow_stored {
        return None;
    }
    if config.remember_password
        && let Some(password) =
            password_from_store(&PasswordStore::from_environment(), url, &email).await
    {
        return Some(ReloginCredentials {
            email,
            password,
            persist: true,
        });
    }
    if io::stdin().is_terminal() {
        let prompt = format!("Session expired. Docmost password for {email}: ");
        let password = rpassword::prompt_password(prompt).ok()?;
        return Some(ReloginCredentials {
            email,
            password: SecretString::from(password),
            persist: true,
        });
    }
    None
}

async fn run_authenticated(
    path: &PathBuf,
    url: &str,
    config: &mut Config,
    allow_stored: bool,
    command: &Command,
    output: Output,
) -> Result<(), AppError> {
    let environment_access = std::env::var("DOCMOST_AUTH_TOKEN").ok();
    let access_from_environment = environment_access.is_some();
    let access_token = environment_access.or_else(|| config.auth_token.clone());
    let mut last_error = None;

    // Stage 1: the current session token, unless its JWT `exp` already
    // passed. With no token at all the command still runs so argument
    // validation fails before any network access.
    let access_usable = access_token
        .as_deref()
        .is_none_or(|token| !session::is_expired(token));
    if access_usable {
        let initial = client(url, access_token.as_deref())?;
        match execute_authenticated(&initial, config, command, output).await {
            Ok(()) => return Ok(()),
            Err(error @ AppError::Client(DocmostError::Unauthorized { .. })) => {
                if access_token.is_some() {
                    last_error = Some(error);
                }
            }
            Err(error) => return Err(error),
        }
    }

    // Stage 2: Docmost has no refresh token, so renewal is a fresh login
    // with stored or prompted credentials.
    let Some(credentials) =
        relogin_credentials(config, url, access_from_environment, allow_stored).await
    else {
        return Err(last_error.unwrap_or_else(session_expired));
    };
    let anonymous = client(url, None)?;
    let token = anonymous
        .auth()
        .login(&LoginRequest {
            email: credentials.email.clone(),
            password: credentials.password,
        })
        .await?;
    if credentials.persist {
        config.email = Some(credentials.email);
        config.auth_token = Some(token.expose_secret().to_owned());
        save_config(path, config)?;
    }
    let retry = client(url, Some(token.expose_secret()))?;
    execute_authenticated(&retry, config, command, output).await
}

fn store_password(
    store: &PasswordStore,
    url: &str,
    email: &str,
    password: &SecretString,
) -> Result<(), AppError> {
    store
        .set(url, email, password)
        .map_err(|error| AppError::Config(format!("unable to save password: {error}")))
}

fn forget_password(store: &PasswordStore, config: &Config) -> Result<(), AppError> {
    let (Some(url), Some(email)) = (&config.api_url, &config.email) else {
        return Ok(());
    };
    store
        .delete(url, email)
        .map_err(|error| AppError::Config(format!("unable to remove saved password: {error}")))
}

async fn login(
    cli: &Cli,
    args: &LoginArgs,
    path: &PathBuf,
    config: &mut Config,
    url: &str,
) -> Result<(), AppError> {
    let email = args
        .email
        .clone()
        .ok_or_else(|| AppError::Config("email required via --email or DOCMOST_EMAIL".into()))?;
    let password = if args.password_stdin {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        text.trim_end().to_owned()
    } else if let Ok(password) = std::env::var("DOCMOST_PASSWORD") {
        password
    } else {
        rpassword::prompt_password("Docmost password: ")?
    };
    let password = SecretString::from(password);
    let anonymous = client(url, None)?;
    let token = anonymous
        .auth()
        .login(&LoginRequest {
            email: email.clone(),
            password: password.clone(),
        })
        .await?;
    let store = PasswordStore::from_environment();
    let previous_email = config.email.replace(email.clone());
    if config.remember_password
        && let Some(previous) = previous_email
        && (previous != email || config.api_url.as_deref() != Some(url))
    {
        // The saved entry belongs to another account or server.
        store
            .delete(config.api_url.as_deref().unwrap_or(url), &previous)
            .ok();
        config.remember_password = false;
    }
    config.api_url = Some(url.to_owned());
    config.auth_token = Some(token.expose_secret().to_owned());
    let remember = args.remember || config.remember_password;
    let stored = if remember {
        store_password(&store, url, &email, &password)
    } else {
        Ok(())
    };
    config.remember_password = remember && stored.is_ok();
    save_config(path, config)?;
    stored?;
    emit(
        cli.output,
        &json!({
            "email": email,
            "api_url": config.api_url,
            "password_stored": config.remember_password,
        }),
    )
}

async fn logout(cli: &Cli, path: &PathBuf, config: &mut Config, url: &str) -> Result<(), AppError> {
    // Best effort: revoke the server-side session when the token may still
    // be valid, then always drop the local credentials.
    let revoked = match config.auth_token.as_deref() {
        Some(token) if !session::is_expired(token) => {
            match client(url, Some(token))?.auth().logout().await {
                Ok(()) => true,
                Err(DocmostError::Unauthorized { .. }) => false,
                Err(error) => {
                    eprintln!("warning: unable to revoke the server session: {error}");
                    false
                }
            }
        }
        _ => false,
    };
    let forgotten = if config.remember_password {
        forget_password(&PasswordStore::from_environment(), config)
    } else {
        Ok(())
    };
    config.auth_token = None;
    config.email = None;
    config.remember_password = false;
    save_config(path, config)?;
    forgotten?;
    emit(
        cli.output,
        &json!({"logged_out": true, "session_revoked": revoked}),
    )
}

async fn run(cli: Cli) -> Result<(), AppError> {
    validate(&cli.command)?;
    let path = config_path()?;
    let mut config = load_config(&path)?;
    let api_overridden = cli.api_url.is_some() || std::env::var_os("DOCMOST_API_URL").is_some();
    match &cli.command {
        Command::Auth(AuthCommand {
            action: AuthAction::Login(args),
        }) => {
            let url = api_url(&cli.api_url, &config)?;
            login(&cli, args, &path, &mut config, &url).await
        }
        Command::Auth(AuthCommand {
            action: AuthAction::Logout,
        }) => {
            let url = api_url(&cli.api_url, &config)?;
            logout(&cli, &path, &mut config, &url).await
        }
        Command::Auth(AuthCommand {
            action: AuthAction::Forget,
        }) => {
            forget_password(&PasswordStore::from_environment(), &config)?;
            config.remember_password = false;
            save_config(&path, &config)?;
            emit(cli.output, &json!({"password_forgotten": true}))
        }
        _ => {
            let url = api_url(&cli.api_url, &config)?;
            run_authenticated(
                &path,
                &url,
                &mut config,
                !api_overridden,
                &cli.command,
                cli.output,
            )
            .await
        }
    }
}

fn main() {
    let cli = Cli::parse();
    let code = match tokio::runtime::Runtime::new().unwrap().block_on(run(cli)) {
        Ok(()) => 0,
        Err(AppError::Usage(e)) => {
            eprintln!("{e}");
            2
        }
        Err(AppError::Config(e)) => {
            eprintln!("{e}");
            3
        }
        Err(AppError::Client(DocmostError::RateLimited { .. })) => {
            eprintln!("Docmost rate limit reached, wait before retrying");
            7
        }
        Err(AppError::Client(DocmostError::PayloadTooLarge { message })) => {
            eprintln!(
                "request too large: {message}; the JSON body limit is 1 MiB, use `page import` for large files"
            );
            5
        }
        Err(e) => {
            eprintln!("{e}");
            5
        }
    };
    std::process::exit(code);
}
