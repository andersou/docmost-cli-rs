# Roadmap

Documentation only: the Docmost API surface that docmost-cli does not cover yet, what each endpoint takes, and what is deliberately out of scope. Paths are relative to `/api`; every RPC-style route is a `POST` with a JSON body unless noted. The inventory was taken from the Docmost `main` branch on 2026-09-04 (v0.95.0 plus unreleased commits) and verified against a v0.95.0 server where noted. Fields marked `pag.` accept the cursor pagination object `{ limit (1-100, default 20), cursor?, beforeCursor?, query? }` and answer `{ items, meta }`.

Nothing here is promised; the list exists so future work starts from facts instead of a fresh reverse-engineering pass.

## Community edition, REST, not implemented yet

### Spaces

| Endpoint | Body | Candidate command |
|---|---|---|
| `spaces/create` | `name` (2-100), `slug` (2-100, `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`), `description?` | `space create --name --slug [--description]` |
| `spaces/update` | `spaceId` (uuid), `name?`, `description?`, `slug?`, `disablePublicSharing?`, `allowViewerComments?` | `space edit` |
| `spaces/delete` | `spaceId` | `space delete --yes` |
| `spaces/members/add` | `spaceId`, `role` (`admin\|writer\|reader`), `userIds` (uuid[], max 25), `groupIds` (uuid[], max 25); both arrays are required, may be empty | `space members add` |
| `spaces/members/remove` | `spaceId`, exactly one of `userId` / `groupId`; refuses to remove the last admin | `space members remove` |
| `spaces/members/change-role` | `spaceId`, `userId?` or `groupId?`, `role` | `space members change-role` |
| `spaces/watch`, `spaces/unwatch`, `spaces/watch-status` | `spaceId` → `{ watching }` | `space watch` |
| `spaces/watched-ids` | none | |

### Groups

| Endpoint | Body | Candidate command |
|---|---|---|
| `groups/create` | `name` (2-100), `description?`, `userIds?` (uuid[], max 50) | `group create` |
| `groups/update` | `groupId`, `name?`, `description?`; the default `Everyone` group cannot be updated | `group edit` |
| `groups/delete` | `groupId`; default group cannot be deleted | `group delete --yes` |
| `groups/members/add` | `groupId`, `userIds` (uuid[], 1-50) | `group members add` |
| `groups/members/remove` | `groupId`, `userId`; not on the default group | `group members remove` |

### Workspace and users

| Endpoint | Body | Notes |
|---|---|---|
| `workspace/public` | none, no auth | `{ id, name, logo, hostname, enforceSso, plan, authProviders }`; useful for a `doctor` command that inspects a server before login |
| `workspace/entitlements` | none | `{ cloud, tier, features }` |
| `workspace/update` | `name?`, `description?`, `emailDomains?`, `trashRetentionDays?` (int ≥ 1), `allowMemberTemplates?`, `allowPersonalSpaces?`, `defaultPageEditMode?` (`read\|edit`), `disablePublicSharing?`; EE-gated flags also exist (`enforceSso`, `enforceMfa`, `mcpEnabled`, `isScimEnabled`, `aiSearch`, `generativeAi`, `aiChat`, `enforceMcpOauth`) | `workspace edit` |
| `workspace/members/deactivate`, `/activate`, `/delete` | `userId` | `workspace members deactivate\|activate\|remove` |
| `workspace/members/change-role` | `userId`, `role` (`owner\|admin\|member`) | |
| `workspace/invites` | `pag.` (`query` filters email) | `workspace invites list` |
| `workspace/invites/create` | `emails` (1-50), `groupIds?` (max 25), `role` (`admin\|member`) | `workspace invites create` |
| `workspace/invites/resend`, `/revoke` | `invitationId` | |
| `workspace/invites/link` | `invitationId` → `{ inviteLink }`; 403 on cloud | |
| `workspace/invites/info`, `/accept` | public; `accept` takes `invitationId`, `name`, `password`, `token` | not a CLI concern |
| `users/update` | `name?` (1-50, no URLs), `email?` (+ `confirmPassword`), `fullPageWidth?`, `pageEditMode?` (`read\|edit`), `editorToolbar?`, `locale?`, notification flags (`notificationPageUpdates`, `notificationPageUserMention`, `notificationCommentUserMention`, `notificationCommentCreated`, `notificationCommentResolved`) | `user edit` |
| `auth/change-password`, `auth/forgot-password`, `auth/password-reset`, `auth/verify-token` | | `auth change-password` |
| `attachments/upload-image` (multipart) | `type` (avatar, workspace logo, space icon), `spaceId?` for space icons, file; size limit `MAX_AVATAR_SIZE` | `user avatar`, `space icon` |
| `attachments/remove-icon` | | |
| `version` | none → `{ currentVersion, latestVersion, releaseUrl }`; 404 on cloud | already used by `auth status`; could surface `latestVersion` |
| `GET health`, `GET health/live` | no auth | part of a `doctor` command |

### Pages

| Endpoint | Body | Notes |
|---|---|---|
| `pages/attachments` | `pageId` + `pag.` | Implemented as `page attachments`, but the route only exists on servers newer than v0.95.0 |
| `pages/labels` | `pageId` + `pag.` | labels are a newer feature (migration `20260509-labels`) |
| `pages/labels/add` | `pageId`, `names` (1-25, normalized to lowercase, `^[a-z0-9_-][a-z0-9_~-]*$`) | `page label add` |
| `pages/labels/remove` | `pageId`, `labelId` | `page label remove` |
| `labels` | `type: "page"` + `pag.` | `label list` |
| `labels/pages` | `labelId?` or `name?`, `spaceId?` + `pag.` | `label pages`; `search --label` already filters by label id |
| `pages/created-by-user` | `userId?`, `spaceId?` + `pag.` | `page list --creator` |
| `pages/backlinks-count` | `pageId` | cheap counter next to `page backlinks` |
| `pages/watch`, `pages/unwatch`, `pages/watch-status` | `pageId` → `{ watching }` | `page watch` |
| `pages/import-zip` (multipart) | `spaceId`, `source` (`generic\|notion\|confluence`), `.zip` file; limit `FILE_IMPORT_SIZE_LIMIT` (default 200mb) → file task | `page import-zip`, ideally with `--wait` polling |
| `file-tasks` | `pag.`; needs workspace Manage Settings | |
| `file-tasks/info` | `fileTaskId` | poll import progress |
| transclusions (`core/page/transclusion`) | `references: [{ sourcePageId, transclusionId }]` (max 50) | not inventoried in detail |
| page-level permissions (`core/page/page-access`) | `PageAccessLevel.restricted`, roles `reader\|writer` | no controller found; possibly EE |

### Comments

- The v0.95.0 controller exposes only create, list, info, update, delete. Resolving (`resolvedAt`, `resolvedById`) happens through the editor; watch for a REST route.
- Inline comment highlighting needs `yjsSelection` (Yjs relative positions), which requires a Yjs client. Out of scope by design; the CLI stores the plain `selection` text only.

### Public sharing

| Endpoint | Body | Notes |
|---|---|---|
| `shares` | `pag.` | own shares, with `page`, `space`, `creator` |
| `shares/create` | `pageId`, `includeSubPages?`, `searchIndexing?`; 400 "Cannot share a restricted page", 403 "Public sharing is disabled" | `share create` |
| `shares/update` | `shareId`, `pageId?`, `includeSubPages?`, `searchIndexing?` | `share edit` |
| `shares/delete` | `shareId` | `share delete --yes` |
| `shares/for-page` | `pageId` → share or no `data` | `share get --page` |
| `shares/info`, `shares/page-info`, `shares/tree`, `search/share-search` | public; `shareId` (or `pageId` for page-info) | anonymous read mode |
| `GET files/public/:fileId/:fileName?jwt=` | public file access for shared pages | |

### Favorites, notifications, search, sessions

| Endpoint | Body | Candidate command |
|---|---|---|
| `favorites/add`, `favorites/remove` | `type` (`page\|space\|template`), matching `pageId` / `spaceId` / `templateId` | `favorite add\|remove` |
| `favorites/ids` | `type`, `spaceId?` → `{ items: string[] }` (max 250) | |
| `favorites` | `type?`, `spaceId?` + `pag.` | `favorite list` |
| `notifications` | `type` (`direct\|updates\|all`, default all) + `pag.` | `notification list` (mirrors taiga-cli) |
| `notifications/unread-count` | none → `{ count }` | `notification count` |
| `notifications/mark-read` | `notificationIds?` (uuid[]) | `notification read` |
| `notifications/mark-all-read` | none | `notification read-all` |
| `search/suggest` | `query`, `includeUsers?`, `includeGroups?`, `includePages?`, `spaceId?`, `limit?` (10) → `{ users, groups, pages }` | `search suggest`; handy to resolve user ids for `--creator` |
| `sessions` | none → `{ sessions: [{ id, deviceName, geoLocation, lastActiveAt, createdAt, isCurrentDevice }] }` | `auth sessions` |
| `sessions/revoke` | `sessionId` (not the current one) | `auth revoke` |
| `sessions/revoke-all` | none; 400 when authenticated without a session (API key) | `auth revoke-all` |

Notification types seen in the source: `comment.user_mention`, `comment.created`, `comment.resolved`, `page.user_mention`, `page.permission_granted`, `page.updated`, plus EE-only `page.verification_*`, `page.approval_*`, `siem_destination.*`.

## Enterprise-only, will not be implemented

API keys (`JwtType.API_KEY`, `ApiKeyService` in the `ee` submodule), OAuth 2.0 and the built-in MCP server (`@OAuthScope`, `/.well-known/oauth-*`, `/mcp`), MFA (`mfa_token`), SSO (`/api/sso/*`), SCIM, audit log and SIEM destinations, page verification and approvals, Typesense search (`SEARCH_DRIVER=typesense`), AI features, billing, and cloud multi-workspace routes (`workspace/create`, `workspace/joined`, `workspace/find-by-email`, hostname resolution).

## CLI improvements that need no new endpoint

- `--output human` with tables; both outputs print JSON today, like taiga-cli-rs.
- `page tree --depth N` and `page move --before <sibling>`.
- Retry with backoff on 429 for non-login calls (`Retry-After` is already parsed).
- Shell completions (`clap_complete`) and man pages (`clap_mangen`).
- Streaming multipart uploads; files are read into memory today.
- A `doctor` command: `GET health` + `workspace/public` + `version`, checking the v0.70.0 minimum before any write.
- Reusing already uploaded attachments on repeated `page edit --upload-local-files` runs (dedupe by content hash) instead of uploading again.

## Known server-side quirks worth tracking

- JSON bodies are limited to 1 MiB (Fastify default); large documents must go through `pages/import`.
- Single-page exports come as `application/octet-stream`; the CLI relies on the `Content-Disposition` file name.
- Markdown export renders the table header row as a regular row.
- The markdown converter keeps only the callout types `info`, `success`, `warning`, `danger`; other names become `info`.
- `attachment info` returns `url: null` on v0.95.0; newer servers add the absolute URL.
