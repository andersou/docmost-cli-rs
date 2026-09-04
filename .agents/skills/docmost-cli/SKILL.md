---
name: docmost-cli
description: Use the docmost-cli command to read and manage a self-hosted Docmost wiki (spaces, pages and their markdown content, attachments, comments, search) from the terminal. Use when asked to inspect, create, update, move, export, or comment on Docmost pages.
---

# docmost-cli

Rust CLI for the Docmost REST API (community edition, server v0.70.0 or newer). Output is pretty-printed JSON; pass `--output json` for guaranteed machine-readable stdout (errors always go to stderr). Page content is written through REST only; the server applies it to the live collaborative document, so no WebSocket or Yjs work is needed.

## Binary

If `docmost-cli` is not on PATH, do not build anything on your own — tell the user to download a precompiled archive from the GitHub releases page (https://github.com/andersou/docmost-cli-rs/releases) for their platform (Linux x86_64, Windows x86_64, Intel macOS, or Apple Silicon macOS) and place the extracted `docmost-cli` binary on PATH.

Only if the user prefers building from source, any Rust 1.98.0+ toolchain works — rustup, vfox (the repo ships a `.vfox.toml`), or another manager:

```sh
cargo build --release --package docmost-cli
```

Binary: `target/release/docmost-cli`.

## Server and authentication

- There is no default server: pass `--api-url https://wiki.example.com` (with or without `/api`) or set `DOCMOST_API_URL`; after `auth login` the URL is remembered.
- Log in once; the session token persists with mode 0600 in the platform config dir (macOS `~/Library/Application Support/docmost-cli/config.json`, Linux `~/.config/docmost-cli/config.json`) or in `DOCMOST_CONFIG` when that variable is set.

```sh
docmost-cli --api-url https://wiki.example.com auth login --email you@example.com   # prompts for password
docmost-cli auth login --email you@example.com --remember          # also saves the password in the OS keychain
printf '%s' "$DOCMOST_PASSWORD" | docmost-cli auth login --email "$DOCMOST_EMAIL" --password-stdin
docmost-cli auth status                                            # identity, server_version, password_stored
docmost-cli auth forget                                            # remove the saved password, keep the session
docmost-cli auth logout                                            # revoke the session, remove token and password
```

- `login` also reads `DOCMOST_EMAIL` and `DOCMOST_PASSWORD` from the environment.
- `DOCMOST_AUTH_TOKEN` overrides the stored token for a single process and is never persisted.
- Sessions renew automatically: when the JWT `exp` passes or the server rejects the token, the CLI logs in again using `DOCMOST_PASSWORD`, the keychain password saved by `--remember`, or a terminal prompt. Without any of those it exits 5 with `no valid session, run \`docmost-cli auth login\``; in non-interactive runs set `DOCMOST_PASSWORD` or ask the user to run `docmost-cli auth login --remember` once.
- Stored credentials are only used for the server in the config file; with `--api-url` or `DOCMOST_API_URL` pointing elsewhere, log in explicitly.
- Accounts that require MFA cannot log in through the API (enterprise feature); the CLI reports it clearly.

## Exit codes

`0` success · `2` usage error · `3` config error · `5` API/other error · `7` server rate limit (wait, then retry).

## IDs and pagination

- Commands take UUIDs. Pages also accept the `slugId` from a page URL (`https://wiki/s/<space>/p/<slugId>-<title>`), and spaces accept their slug.
- Lists paginate by cursor: `--limit N` (1-100), `--cursor <meta.nextCursor>`, `--query TEXT`, or `--all` to follow every cursor. JSON output is `{"items": [...], "meta": {...}}`.
- Find the space first, then navigate pages:

```sh
docmost-cli space list --all --output json | jq '.items[] | {id, name, slug}'
docmost-cli page tree --space <SPACE_ID>                    # root pages
docmost-cli page tree --space <SPACE_ID> --recursive        # whole tree, items carry "depth"
docmost-cli page tree --parent <PAGE_ID>                    # children of one page
docmost-cli page list --space <SPACE_ID> --all              # recently updated pages
docmost-cli search "release checklist" --space <SPACE_ID>   # full-text; --title-only, --limit, --offset
```

## Reading pages

```sh
docmost-cli page get <PAGE_ID>                    # metadata + "content" as markdown (default)
docmost-cli page get <PAGE_ID> --content html     # or json (ProseMirror), or none
docmost-cli page get <PAGE_ID> --include-space
docmost-cli page export <PAGE_ID> --format markdown > page.md
docmost-cli page export <PAGE_ID> --format html --include-children --include-attachments --out subtree.zip
docmost-cli page breadcrumbs <PAGE_ID>
docmost-cli page backlinks <PAGE_ID> --direction incoming
docmost-cli page history <PAGE_ID>; docmost-cli page history-get <HISTORY_ID>
docmost-cli page attachments <PAGE_ID>            # needs a server newer than v0.95.0
```

## Writing pages

Content comes from `--content "text"`, `--content-file path`, or `--content-file -` (stdin); `--format markdown` (default), `html`, or `json` (ProseMirror document).

```sh
docmost-cli page create --space <SPACE_ID> --title "Runbook" --content-file runbook.md
docmost-cli page create --space <SPACE_ID> --title "Child" --parent <PAGE_ID> --content "# Draft"
docmost-cli page edit <PAGE_ID> --content-file new.md                      # replace the whole body
docmost-cli page edit <PAGE_ID> --content-file - --operation append < extra.md
docmost-cli page edit <PAGE_ID> --title "New title"                        # metadata only
docmost-cli page move <PAGE_ID> --parent <PARENT_ID>                       # last among siblings
docmost-cli page move <PAGE_ID> --root --first                             # or --after <SIBLING_ID>, or --position <KEY>
docmost-cli page move-to-space <PAGE_ID> --space <SPACE_ID>
docmost-cli page duplicate <PAGE_ID>
docmost-cli page delete <PAGE_ID> [<PAGE_ID>...] --yes                     # trash; refuses without --yes
docmost-cli page delete <PAGE_ID> --yes --permanent
docmost-cli page restore <PAGE_ID>; docmost-cli page trash --space <SPACE_ID>
docmost-cli page import --space <SPACE_ID> --file notes.docx --parent <PARENT_ID>   # .md .html .docx .pdf
```

- `--data '<json object>'` on `create`/`edit` merges arbitrary API fields; explicit flags win.
- Markdown is converted by the server. Supported: headings, lists, task lists, tables, fenced code, `$math$`/`$$math$$`, footnotes, and callouts written as `:::info` … `:::` (types `info`, `success`, `warning`, `danger`; others become `info`). Status badges use the CLI shorthand `<status color="green">TEXT</status>` (colors gray, blue, green, yellow, red, purple). Nested callouts and raw HTML blocks are not reliable.
- JSON bodies are limited to 1 MiB by the server; for very large documents use `page import`.

## Attachments

```sh
docmost-cli page create --space <SPACE_ID> --title "Design" --content-file design.md --upload-local-files
docmost-cli page edit <PAGE_ID> --content-file design.md --upload-local-files --attachments-base ./assets
docmost-cli page attach <PAGE_ID> --file diagram.png --file spec.pdf     # image/video nodes or download cards, appended
docmost-cli page attach <PAGE_ID> --file spec.pdf --no-insert            # upload only
docmost-cli attachment info <ATTACHMENT_ID>
docmost-cli attachment download <ATTACHMENT_ID> <FILE_NAME> --out spec.pdf
```

`--upload-local-files` scans markdown links and images whose target is a local path (relative to the content file's directory, or `--attachments-base`), uploads them, and rewrites the links to `/api/files/<id>/<name>`. Images become embedded images; other files become plain links, so use `page attach` when a download card is wanted.

## Comments

```sh
docmost-cli comment list --page <PAGE_ID> --all
docmost-cli comment create --page <PAGE_ID> --text "Looks good"
docmost-cli comment create --page <PAGE_ID> --text "Typo here" --selection "exact page text"   # inline comment
docmost-cli comment create --page <PAGE_ID> --text "Agreed" --parent <COMMENT_ID>            # reply
docmost-cli comment edit <COMMENT_ID> --text "Updated"
docmost-cli comment delete <COMMENT_ID> --yes
```

`--content-json` accepts a raw ProseMirror document instead of `--text`. Inline comments are stored with their selection but are not highlighted in the editor.

## Workspace, spaces, groups, users

```sh
docmost-cli workspace info; docmost-cli workspace members --all
docmost-cli space get <SPACE_ID_OR_SLUG>; docmost-cli space members <SPACE_ID>
docmost-cli space export <SPACE_ID> --format markdown --out space.zip
docmost-cli group list; docmost-cli group members <GROUP_ID>
docmost-cli user me
```

## Automation

```sh
docmost-cli page tree --space <SPACE_ID> --recursive --output json | jq '.items[] | {id, slugId, title, depth}'
docmost-cli page get <PAGE_ID> --output json | jq -r .content > page.md
docmost-cli page create --space <SPACE_ID> --title "Report" --content-file report.md --output json | jq -r .id
```
