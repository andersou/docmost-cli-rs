# docmost-cli-rs

Rust CLI and reusable async client for the [Docmost](https://docmost.com) wiki API. It targets self-hosted Docmost community edition servers, authenticates with email and password, writes page content through the REST API only, and prints JSON for automation.

## Features

- Email and password authentication with private local session storage and silent re-login when the session expires or is revoked.
- Workspace, spaces, groups, and members; pages with markdown, HTML, or ProseMirror JSON content (create, replace, append, prepend); page tree, move, duplicate, trash, restore, history, breadcrumbs, and backlinks.
- File attachments: upload local files referenced from markdown, attach files as image, video, or download cards, list, inspect, and download attachments.
- Page and space export (markdown, HTML, zip) and import of `.md`, `.html`, `.docx`, and `.pdf` files.
- Comments (page-level, inline, and replies) and full-text search.
- Precompiled binaries published as GitHub release archives for Linux x86_64, Windows x86_64, Intel macOS, and Apple Silicon macOS.

## Requirements

- A Docmost server at **v0.70.0 or newer**. Page content is written through `pages/create` and `pages/update`, which learned the `content`, `format`, and `operation` fields in that release; older servers silently drop those fields and create empty pages. `docmost-cli auth status` prints the server version.
- Community edition is enough. API keys, OAuth, and MFA are enterprise features and are not used; accounts that require MFA cannot log in through the API.
- `page attachments` (listing the files of a page) needs a server newer than v0.95.0; every other command works on v0.70.0 and later.
- Rust 1.98.0 or newer to build from source. Install it however you prefer: [rustup](https://rustup.rs/) or any other toolchain manager works. If you use [vfox](https://vfox.lhan.me/), the repository ships a `.vfox.toml` and the command below selects the pinned toolchain, but vfox is only a suggestion, not a requirement.

```sh
# optional, only if you use vfox:
vfox use -p rust@1.98.0
```

## Install

Download a precompiled archive from the [GitHub releases](https://github.com/andersou/docmost-cli-rs/releases) page and place the `docmost-cli` binary on your `PATH`. Each release ships archives for the four supported targets plus a `SHA256SUMS` file for verification.

### Agent skill

Install the global skill from a local clone:

```sh
npx skills add /path/to/docmost-cli-rs --global --skill docmost-cli --yes
```

Or install directly from GitHub:

```sh
npx skills add https://github.com/andersou/docmost-cli-rs --global --skill docmost-cli --yes
```


To build from source instead:

```sh
cargo test --workspace --all-targets --all-features --locked
cargo build --release --package docmost-cli
```

The binary is written to `target/release/docmost-cli`.

## Authentication

Log in interactively; the server URL is accepted with or without the `/api` suffix:

```sh
docmost-cli --api-url https://wiki.example.com auth login --email you@example.com
```

For scripts, pass credentials through the environment and read the password from standard input:

```sh
export DOCMOST_API_URL=https://wiki.example.com
printf '%s' "$DOCMOST_PASSWORD" | docmost-cli auth login --email "$DOCMOST_EMAIL" --password-stdin
```

The session token, email, and API URL are saved with mode `0600` to the platform configuration directory (macOS `~/Library/Application Support/docmost-cli/config.json`, Linux `~/.config/docmost-cli/config.json`), or to `DOCMOST_CONFIG` when that variable is set. `DOCMOST_AUTH_TOKEN` overrides the stored token for a single process and is never persisted.

### Session renewal

Docmost issues one session JWT (30 days by default) and no refresh token. The CLI keeps the session usable without wasted round trips:

1. The stored token is used while its JWT `exp` claim is still valid; an expired one is skipped instead of producing a failed request.
2. When the token is expired or the server rejects it, the CLI logs in again with, in order: `DOCMOST_PASSWORD` from the environment, the password saved with `--remember`, or an interactive prompt when stdin is a terminal. Otherwise it fails with `no valid session, run \`docmost-cli auth login\``.

Re-login with stored or prompted credentials only happens for the server saved in the configuration; with `--api-url` or `DOCMOST_API_URL` pointing elsewhere, or with `DOCMOST_AUTH_TOKEN` set, stored credentials are never sent. Logins are rate limited by the server, which is why the session is cached instead of logging in on every command.

To skip the prompt entirely, save the password in the system keychain (macOS Keychain, Windows Credential Manager, or the Secret Service on Linux):

```sh
docmost-cli auth login --email you@example.com --remember
docmost-cli auth forget    # drop the saved password, keep the session
docmost-cli auth logout    # revoke the session on the server, drop the token and the saved password
```

Headless Linux hosts without a Secret Service daemon cannot store passwords; use `DOCMOST_PASSWORD` there instead.

## Commands

```sh
docmost-cli --help
docmost-cli auth status                                   # identity, server version, config location
docmost-cli space list --all
docmost-cli page tree --space <SPACE_ID> --recursive
docmost-cli page get <PAGE_ID_OR_SLUG_ID>                 # metadata plus markdown content
docmost-cli page create --space <SPACE_ID> --title "Runbook" --content-file runbook.md --upload-local-files
docmost-cli page edit <PAGE_ID> --content-file - --operation append < notes.md
docmost-cli page move <PAGE_ID> --parent <PARENT_ID>       # default: last among the new siblings
docmost-cli page attach <PAGE_ID> --file diagram.png --file spec.pdf
docmost-cli page export <PAGE_ID> --format markdown > page.md
docmost-cli page import --space <SPACE_ID> --file notes.docx --parent <PARENT_ID>
docmost-cli page delete <PAGE_ID> --yes                   # trash; add --permanent to skip it
docmost-cli comment create --page <PAGE_ID> --text "Looks good"
docmost-cli search "deployment checklist" --space <SPACE_ID>
```

Every command accepts `--output human` (default) or `--output json`; both print pretty JSON today. Success output is written to stdout; errors are written to stderr with a nonzero exit status. `page export` and `attachment download` write raw file bytes to stdout unless `--out` is given.

Pages accept either their UUID or the `slugId` from the page URL (`/s/<space>/p/<slugId>-<title>`); spaces accept their UUID or slug. Lists paginate by cursor: `--limit`, `--cursor <meta.nextCursor>`, `--query`, or `--all` to follow every cursor.

### Exit codes

`0` success · `2` usage error · `3` configuration error · `5` API or other error · `7` server rate limit (wait, then retry).

## Markdown notes

Markdown is converted by the server, so anything Docmost imports correctly works here: headings, lists, task lists, tables, fenced code, `$math$` and `$$math$$`, footnotes, and callouts:

```markdown
:::warning
Callout types: info, success, warning, danger (other names become info).
:::

<status color="green">READY</status>
```

`<status color="...">` is a CLI shorthand for the Docmost status badge (`gray`, `blue`, `green`, `yellow`, `red`, `purple`); it is rewritten before upload. With `--upload-local-files`, images referenced as `![alt](path)` become embedded images and other local links become plain links to the uploaded file; use `page attach` for download cards. JSON request bodies are limited to 1 MiB by the server, so very large documents go through `page import`.

## Releases

Conventional Commits determine releases:

- `feat`: minor release.
- `fix`, `perf`, `revert`: patch release.
- `!` or a `BREAKING CHANGE` footer: major release.
- `docs`, `chore`, `ci`, `test`, `refactor`, `style`, and `build`: no release unless breaking.

GitHub Actions tests the workspace and creates native archives for the four supported targets. Pushes to `main` create stable releases; pushes to `develop` create `beta` prereleases. Each release contains archives and `SHA256SUMS`.

Run the release planner locally after installing Node dependencies:

```sh
npm ci --ignore-scripts
npm run release:dry-run -- --no-ci
```

Publishing is CI-only and requires `EXPECTED_VERSION`.

## Local smoke test

`scripts/docmost-smoke-compose.yml` starts a throwaway Docmost (with PostgreSQL and Redis) on port 3300 for end-to-end checks:

```sh
docker compose -f scripts/docmost-smoke-compose.yml -p docmost-smoke up -d
curl -sS -X POST http://localhost:3300/api/auth/setup -H 'content-type: application/json' \
  -d '{"workspaceName":"Smoke","name":"Ada","email":"ada@smoke.test","password":"smoke-pass-123"}' >/dev/null
export DOCMOST_CONFIG=/tmp/docmost-smoke.json
printf '%s' smoke-pass-123 | docmost-cli --api-url http://localhost:3300 auth login --email ada@smoke.test --password-stdin
docmost-cli auth status
docker compose -f scripts/docmost-smoke-compose.yml -p docmost-smoke down -v
```

## Quality hooks

[prek](https://prek.j178.dev/) runs repository checks, workflow linting, Conventional Commit validation, formatting, Clippy, and pre-push tests.

```sh
prek install
prek run --all-files
prek run cargo-test --hook-stage pre-push --all-files
```

## Migrating from docmost-mcp

| MCP tool | Command |
|---|---|
| `get_workspace` | `docmost-cli workspace info` |
| `list_spaces` | `docmost-cli space list --all` |
| `list_groups` | `docmost-cli group list --all` |
| `list_pages` | `docmost-cli page list --space <ID> --all` |
| `get_page` | `docmost-cli page get <ID>` (subpages: `page tree --parent <ID>`) |
| `create_page` | `docmost-cli page create --space <ID> --title T --content-file F [--parent P] [--upload-local-files --attachments-base DIR]` |
| `update_page` | `docmost-cli page edit <ID> --content-file F [--title T] [--operation replace\|append\|prepend]` |
| `move_page` | `docmost-cli page move <ID> --parent <P>` or `--root` |
| `delete_page` / `delete_pages` | `docmost-cli page delete <ID>... --yes` |
| `search` | `docmost-cli search <QUERY> [--space <ID>]` |
| `create_comment` | `docmost-cli comment create --page <ID> --text T [--selection S] [--parent C]` |
| `list_page_comments` | `docmost-cli comment list --page <ID> [--all]` |
| `get_comment` / `update_comment` / `delete_comment` | `docmost-cli comment get\|edit\|delete <ID>` |

Content is written through REST only. Docmost applies the change through its own collaboration server, so open editors see it live and the stored document stays consistent. Inline comments are created without the editor highlight the MCP added over WebSocket.

## Roadmap

[ROADMAP.md](ROADMAP.md) lists the rest of the Docmost API surface (spaces and groups administration, labels, favorites, notifications, public shares, sessions, zip imports), what is enterprise-only, and CLI improvements under consideration.

## License

MIT. See [LICENSE](LICENSE).
