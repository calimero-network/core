# wiki

A team wiki on the component SDK (`calimero-sdk`, built for `wasm32-wasip2` into a
`calimero:app` component). It shows several SDK features in one small app:

- **`#[app::init]` seeding the first state**: `init` runs once, as the app's owner, and
  writes the `home` page with the owner as its author, so a member's first read sees it.
- **A custom key type**: pages live in an `UnorderedMap<Slug, Page>`. `Slug` is a
  `ToKeyPart` newtype over the title trimmed, lowercased and with each run of whitespace
  a `-`, so `"Getting  Started "` and `getting-started` name one page. A struct without
  `ToKeyPart` cannot be a key; `src/lib.rs` says why.
- **Author-owned pages**: the first to create a slug owns it (`PageExists` after). Only the
  author revises, renames, tags, attaches to or deletes it (`NotAuthor { slug }`). Every
  revision is kept under `(slug, number)`. A rename changes the title, never the slug.
- **A sorted tag set**: `Set<String>` holds every tag in use, in order: autocomplete
  (`tagsStartingWith`), an index slice (`tagsBetween`), paging (`tags`) and the count with
  the first and last (`tagStats`) each read a slice of it. A tag leaves the set with its last
  page.
- **Per-person drafts**: a `PerUser<Draft>`. The setters take no account, so an account
  writes only its own draft; anyone reads one with `draftOf(account)`.
- **Content-addressed attachments**: a `Frozen<String, PlainSha256>` keyed by the SHA-256
  of the content's bytes (what `sha256sum` prints, through a custom `ContentHash`). Equal
  content returns the same hash and stores nothing. Nothing changes or removes stored
  content, even when its page is deleted.

Writes need an org member (or above); guests only read.

## ABI

Names are camelCase. `u64` values travel as decimal strings. Revision numbers are `u32`,
so they are plain JSON numbers. Listings return at most 1000 rows.

| Method | Kind | Params | Returns | Notes |
| --- | --- | --- | --- | --- |
| `init` | init | | | seeds `home`, authored by the owner |
| `me` | view | | `string` | the caller's account |
| `createPage` | call | `title, body` | `string` | the slug; `BadTitle`, `PageExists { slug }` |
| `revisePage` | call | `slug, body` | `u32` | the new revision's number |
| `renamePage` | call | `slug, title` | | the slug stays |
| `deletePage` | call | `slug` | `bool` | drops revisions and tags; `false` if there is no page |
| `page` | view | `slug` | `PageView \| null` | the latest revision, with tags and attachment hashes |
| `history` | view | `slug` | `[{revision, body}]` | oldest first |
| `pages` | view | | `[{slug, title, author}]` | slug order |
| `tagPage` / `untagPage` | call | `slug, tag` | `bool` | `true` when it changed something; `BadTag` |
| `pagesTagged` | view | `tag` | `[string]` | slugs, in order |
| `tagsStartingWith` | view | `prefix` | `[string]` | at most 10 |
| `tagsBetween` | view | `start, end` | `[string]` | `[start, end)` |
| `tags` | view | `offset, limit` | `[string]` | one page of the tag set |
| `tagStats` | view | | `{count, first, last}` | |
| `saveDraft` | call | `title, body` | | the caller's own draft |
| `myDraft` / `draftOf` | view | — / `account` | `Draft \| null` | |
| `discardDraft` | call | | `bool` | |
| `publishDraft` | call | | `string` | creates the page, or revises the caller's own; `NoDraft` |
| `attach` | call | `slug, content` | `string` | the hex SHA-256 |
| `attachment` | view | `hash` | `string` | `BadHash { hash }`, `NoAttachment { hash }` |

Errors: `BadTitle { title }`, `PageExists { slug }`, `NoPage { slug }`, `NotAuthor { slug }`,
`BadTag { tag }`, `NoDraft`, `BadHash { hash }`, `NoAttachment { hash }`; guests' writes are
`Forbidden`. Events, to `Audience::All`: `PageChanged { slug }` for every write to a page (read it
again), and `AttachmentStored { hash }` only for new content. A write that changes nothing
emits nothing.

## Build and test

```bash
cargo test                                         # harness tests and scenarios/*.json
meroctl app build apps/wiki --out wiki.wasm        # from the repo root
```

`tests/harness.rs` tests each rule, with its refusals. `scenarios/wiki.json` covers the
seeded page, slugs, the author rule, revisions and the tag set. `scenarios/drafts-and-attachments.json`
covers drafts and attachments. The cluster runs the same files.
