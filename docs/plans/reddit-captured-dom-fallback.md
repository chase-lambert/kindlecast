# Reddit Captured-DOM Fallback

## Goal

Make Reddit discussions reliably sendable from the browser extension when Reddit rejects KindleCast's unauthenticated JSON request with HTTP 403. The extension must produce a Kindle-ready discussion from the Reddit page already rendered in the user's tab, including post metadata, visible comments, and comment nesting. It must not silently turn login, consent, or bot-block pages into generic articles.

The reported reproduction is `https://www.reddit.com/r/Zig/comments/1urnjvh/...`, whose `comments/1urnjvh.json?raw_json=1&limit=500` endpoint returns 403. A direct check from the development environment returned 403 for both that JSON endpoint and the old Reddit HTML endpoint, confirming that another native-host fetch is not a dependable extension strategy.

## Context

- `extension/popup.js` deliberately skips DOM capture for every recognized discussion site, including Reddit. The native host therefore receives `page_html: null` for Reddit.
- `src/sites/reddit.rs` ignores `page_html` and always requests Reddit's public JSON endpoint. Its JSON decoder and fixtures otherwise already produce the desired `Thread` model.
- `src/native_host.rs` already accepts captured HTML up to 64 MiB, and generic article handling already establishes captured DOM as a supported native-message input.
- `src/sites/mod.rs` routes Reddit URLs to the Reddit adapter before the generic article adapter, so a failed Reddit extraction cannot accidentally become a generic article unless routing is explicitly changed.
- `dom_smoothie` already brings `dom_query 0.28` into the lockfile transitively. Adding it as a direct dependency gives the Reddit adapter documented CSS selection, ancestor traversal, and HTML serialization without introducing a second HTML parser stack.
- Old Reddit exposes conventional `.thing.link` / `.thing.comment` markup. Current desktop Reddit uses `shreddit-post` and `shreddit-comment` elements with useful metadata attributes and slotted post/comment bodies. These layouts need separate, fixture-backed extractors behind one captured-page entry point.
- Existing JSON behavior reports omitted `more` children through progress while `Thread.comment_count` counts only included comments. Captured-page behavior should preserve that convention.

## Design

### Browser capture

Refactor `extension/popup.js` so Reddit URLs are captured like generic pages while Hacker News and Lobsters remain URL-only API adapters. Continue sending the active tab's `document.documentElement.outerHTML` in the existing `pageHtml` field; do not add a new native messaging protocol shape.

Capture remains best-effort at the browser boundary. A missing capture reaches the Reddit adapter as `None`, preserving CLI and capture-failure behavior.

### Reddit fetch strategy

In `Reddit::fetch`:

1. If non-empty `page_html` is present, report a captured-DOM progress message and try captured-page extraction first. This is the normal extension path and avoids an expected 403 on every send.
2. If captured extraction fails, retain its contextual error and try the existing JSON endpoint as a compatibility fallback. If JSON succeeds, return it. If JSON also fails, return one actionable error containing both causes rather than hiding the malformed/blocked capture behind the 403.
3. If no captured HTML is present (including CLI usage), keep the existing JSON path. Improve the 403 context to explain that Reddit rejected unauthenticated access and that browser capture is the supported fallback.
4. Resolve share URLs only when the selected path actually needs an ID. Captured extraction should derive the post ID/permalink from the document where possible, avoiding a second Reddit request for an already-rendered share destination.

Do not route extraction failures to `article::fetch`.

### Captured-page parser

Add `dom_query = "0.28"` as a direct dependency and implement a pure parser in `src/sites/reddit.rs`, factored so fixture tests require no network. It returns the same `(Thread, omitted_count)` shape as the JSON builder.

Detect and parse layouts explicitly:

- **Old Reddit:** root post from `.thing.link` metadata and its own `.entry`; derive the ID by stripping `t3_` from `data-fullname`, and read `data-permalink` before the post entry's `.flat-list a.comments[href]` discussion link or the supplied canonical comments URL. Use `.entry p.title > a.title[href]` only for the submitted content URL, not as the discussion permalink. Extract title, author, score, timestamp, subreddit, and optional `.expando .usertext-body .md` selftext. Parse `.thing.comment` nodes in document order. Use each comment's count of comment ancestors (or its old-Reddit depth class as a checked fallback) as structural depth. Traverse the comment root's immediate children to obtain its own `.entry` rather than using an unrestricted descendant selection, which would include nested comment entries; likewise serialize only that entry's own body.
- **Current desktop Reddit:** root from `shreddit-post`; derive identity in the order `post-id`, `thingid`, then `id`, stripping any `t3_` prefix, and read `permalink`. Read title, author, score, timestamp, content URL, and subreddit/community from rendered attributes with narrowly scoped DOM fallbacks. Parse `shreddit-comment` in document order, use its `depth` attribute (cross-checked/fallback to comment ancestors), and extract only its own `[slot="comment"]` body.

If both roots are present, prefer the current-desktop `shreddit-post` extractor because it represents the active UI; fall back to old Reddit only if current extraction fails validation and a real `.thing.link` root is present. If neither is present, reject the capture. Dedicated `m.reddit.com` markup is out of scope (the accepted URL patterns do not currently include that host); if a responsive/mobile document reaches the adapter without either supported root, fail closed with guidance to open the desktop or old Reddit view.

Normalize the extracted flat comment sequence into `Comment` trees with the same recursive depth-building pattern used by `src/sites/lobsters.rs`. Handle malformed depth jumps by promoting to the nearest valid parent rather than panicking. Promote children of genuinely empty/deleted placeholders to the removed parent's depth, matching current Reddit JSON behavior. Compute `comment_count` and `max_depth` from the resulting tree.

Count recognizable old/current “more comments/replies” placeholders when a numeric count is available and report the total through the existing progress callback. Do not fetch or expand them. Keep `Thread.comment_count` equal to visible extracted comments, consistent with the JSON adapter.

Accept Reddit timestamps in RFC 3339 and numeric epoch forms. Treat absolute numeric values at or above `100_000_000_000` as milliseconds and smaller values as seconds; this cleanly separates plausible Reddit-era values. If a post or individual comment timestamp is absent, use the existing `Utc::now` fallback used by other adapters; required post identity/title failures reject the capture.

Before returning a thread, validate that the document is actually a usable Reddit discussion: require a recognizable Reddit post root, a non-empty title, and stable post identity or permalink. A page with login/consent/block text but no post root must return a specific captured-page extraction error. Zero visible comments is valid for a real post.

### Content handling

Retain the post/selftext and comment body HTML already rendered by Reddit, but select only the content containers described above so vote controls, navigation, reply controls, moderation links, and nested comments are excluded. Remove script/style/template and known interaction-only descendants from extracted content before serialization if those can occur inside the selected body. Continue relying on the existing render pipeline to neutralize headings and produce EPUB-safe structure.

### Documentation and errors

Update `README.md` to replace the note implying the JSON user-agent approach is sufficient. Explain that the extension captures the rendered Reddit discussion because public JSON/HTML commonly returns 403, that only currently rendered comments are included, and that CLI Reddit support still depends on public JSON availability.

Progress/error text should distinguish “using captured Reddit page,” omitted visible placeholders, parser rejection, and Reddit HTTP rejection without exposing a wall of duplicate context in the popup.

Use a stable combined-error shape: `captured Reddit page could not be parsed: <capture cause>; JSON fallback also failed: <JSON cause>`. Keep combination in a small pure helper so unit tests can assert that neither cause is lost without making a live network request.

## Key Decisions

1. **Choose captured DOM first for extension requests, with JSON only as fallback.** Strongest alternative: keep JSON primary and parse the DOM only after a 403. Rationale: both the user's repeated result and local reproduction indicate 403 is normal, so JSON-first adds latency and a known failure to every extension send. The retained fallback preserves environments where JSON still works. **Impact: high; reversibility: easy.**
2. **Parse raw captured HTML in Rust using a direct `dom_query` dependency.** Strongest alternative: run Reddit-specific JavaScript in the tab and send a new structured snapshot over native messaging. Rationale: one protocol already carries HTML, Rust fixture tests can cover selectors and tree construction, and parsing remains centralized with the existing Reddit adapter. Browser-side structured extraction may become attractive if shadow-DOM-only content later proves inaccessible in `outerHTML`. **Impact: medium; reversibility: moderate.**
3. **Maintain separate old-Reddit and current-desktop extractors, sharing normalization.** Strongest alternative: support only the old layout shown in the reproduction. Rationale: old Reddit is the priority, but recognizing current `shreddit-*` markup is a bounded addition and prevents the fallback from being tied to a user preference. Selectors remain explicit rather than a brittle union. **Impact: medium; reversibility: easy.**
4. **Fail closed on non-content pages instead of using generic readability extraction.** Strongest alternative: send whatever Readability extracts. Rationale: login and bot-block pages can contain enough text to pass generic extraction and would create misleading Kindle documents. **Impact: medium; reversibility: easy.**
5. **Include only comments already rendered in the captured DOM and report, but do not expand, `more` placeholders.** Strongest alternative: have the extension click/load every placeholder before capture. Rationale: expansion is slow, UI-dependent, potentially rate-limited, and may make authenticated actions on the user's behalf. This matches the accepted intake default. **Impact: high; reversibility: moderate.**
6. **Leave CLI Reddit behavior API-only.** Strongest alternative: add an unauthenticated HTML fetch fallback. Rationale: direct old Reddit HTML returned the same 403 and the CLI has no logged-in rendered page to contribute, so another endpoint does not solve the access problem. Errors will explain the limitation. **Impact: medium; reversibility: easy.**
7. **Count only extracted comments in rendered metadata.** Strongest alternative: add omitted placeholders to `Thread.comment_count`. Rationale: existing JSON behavior and rendering use the count to describe comments actually present in the book; inflating it would make navigation/content inconsistent. Omitted counts remain progress information. **Impact: low; reversibility: easy.**

## Implementation Steps

1. Add `dom_query 0.28` as a direct dependency in `Cargo.toml`, intentionally aligned with `dom_smoothie 0.18`'s current transitive version, and update `Cargo.lock` through Cargo. Recheck alignment when either dependency is upgraded to avoid duplicate parser versions.
2. Change `extension/popup.js` URL classification so Reddit pages are captured while Hacker News and Lobsters remain capture-free.
3. Refactor `src/sites/reddit.rs` fetch orchestration into captured-first and JSON helper paths with combined, actionable error context.
4. Implement layout detection plus old-Reddit and current-desktop metadata/body extraction using `dom_query`.
5. Implement flat-comment normalization, deleted-placeholder promotion, timestamp/score/URL normalization, omitted-placeholder counting, and final `Thread` validation/statistics.
6. Add compact HTML fixtures for old Reddit, current desktop Reddit, and a blocked/login page under `src/sites/fixtures/`.
7. Add unit tests covering both layouts, metadata, external-link versus self-post behavior, body isolation (no nested comments or controls), nesting/depth jumps, omitted counts, zero-comment posts, blocked-page rejection, and the combined-error helper. Include a captured page whose input URL is a Reddit share URL and verify that post-root `data-fullname`/`post-id` and permalink supply the canonical ID/discussion URL without resolving the share URL over the network. Keep existing JSON and URL tests passing.
8. Update the extension and Reddit notes in `README.md` to describe captured-DOM behavior and CLI limitations.
9. Run formatting, unit tests, and lint checks.

## Test Plan

- `cargo fmt -- --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- Old Reddit fixture assertions:
  - correct ID, title, external URL, author, score, subreddit, timestamp, and discussion URL;
  - visible top-level and nested comments become the correct tree;
  - a deleted empty parent promotes descendants;
  - comment body HTML excludes nested comments and controls;
  - numeric “more replies” entries contribute to the omitted progress count.
- Current desktop fixture assertions for equivalent metadata/tree extraction from `shreddit-*` markup, including a self-post body.
- A recognizable zero-comment Reddit post succeeds with an empty comment list.
- Login/consent/bot-block HTML without a post root is rejected as captured Reddit content and cannot be routed as a generic article.
- A responsive/mobile capture with neither supported root fails with guidance to use desktop or old Reddit rather than producing a generic article.
- A pure combined-error test asserts that both the captured-parser cause and JSON-fallback cause appear in the stable, concise format.
- Existing JSON fixture tests continue to pass, proving API compatibility.
- Manual extension smoke test on the supplied Reddit URL after reloading the unpacked extension and reinstalling/restarting the native host as needed: popup reports captured Reddit extraction, generates an EPUB, and the output contains metadata and the comments visible in the browser despite the JSON endpoint returning 403.

## Risks / Open Questions

- Current Reddit changes custom-element attributes and may render some content only inside runtime shadow roots that `outerHTML` does not serialize. The extractor will support the presently known `shreddit-*` light-DOM representation and fail clearly when required content is absent. A future protocol carrying a browser-produced structured snapshot is the fallback design if real-page validation exposes this limitation.
- Fixture markup is necessarily smaller than Reddit's production DOM. Selectors should anchor to semantic classes/attributes and tests should include surrounding noise to reduce false confidence.
- Very large fully rendered threads can approach the native host's 64 MiB request cap. This change does not increase that cap; the existing native-messaging error remains preferable to unbounded messages.
- Captured HTML is untrusted input. Extraction must scope and clean body containers and must never execute scripts. `dom_query` parses/serializes only; downstream Pandoc receives static HTML.
- Reddit share links may not expose a parseable ID in the original URL. Captured extraction must prefer post-root identity/permalink. API fallback may still fail to resolve a share URL on blocked networks; the combined error should make that clear.
- Parsing a near-limit 64 MiB DOM can be CPU- and memory-intensive. Keep extraction to a single parsed document and avoid cloning the full source; no new timeout is introduced because local parsing remains preferable to silently truncating content.
- **Rejected plan-critique finding — bump `extension/manifest.json` version as part of this implementation.** The repository has no convention of bumping versions per unreleased feature, and the extension and Rust package currently share `0.2.0`; changing only the manifest would create version drift while changing both would turn this bug fix into an unrequested release/versioning decision. Leave versioning to the eventual release change.
