# kindlecast

`kindlecast` turns discussion threads and web articles into Kindle-friendly EPUBs, saves them, and can email them straight to your Kindle.

Supported sources:

- Hacker News item URLs and bare HN IDs
- Reddit comment threads (`reddit.com/comments/...`, `redd.it/...`, and share links)
- Lobsters stories
- Generic `http(s)` articles through Readability-style extraction

## Built for reading big threads on e-ink

Kindles strip all interactivity from documents, so you can't collapse a comment tree the way you would in a browser. kindlecast emulates it with structure instead:

- **One chapter per top-level comment.** Every top-level comment becomes its own chapter, titled `author · first words of the comment…` in the table of contents. On the Kindle, swipe up to open Page Flip and the chapter-skip arrows jump thread-to-thread; tap the top of the screen for the full TOC. Setting the reading footer to "chapter progress" shows how far into the current thread you are.
- **Skip links.** Any comment with 5+ replies beneath it gets a small `skip N replies ↓` link that jumps just past the subtree — one tap to bypass a tangent you're done with.
- **Depth-capped indentation.** Nesting indents up to 5 levels (configurable via `max_indent_depth`); deeper comments stay at the cap with a `↳ depth` marker so walls of margin never eat the page.

Comment ordering matches the site: for HN, ranked order comes from the official Firebase API (see Notes).

## Install

Pandoc is required:

```sh
sudo apt install pandoc   # or: dnf install pandoc / brew install pandoc
```

Then build and install the binary:

```sh
cargo install --path .
```

## CLI

```sh
kindlecast 126809 --no-email
kindlecast 'https://news.ycombinator.com/item?id=126809'
kindlecast 'https://lobste.rs/s/abc123/title' --no-email
kindlecast 'https://example.com/article' --no-email --keep-html
```

The default mode saves to `~/Downloads` and emails the EPUB. Use `--no-email` to build only, or `--email-only` to avoid keeping a copy in Downloads.

## Config

```sh
kindlecast init
```

This writes `~/.config/kindlecast/config.toml` (permissions `0600` — readable only by you, since it holds an email password) and copies `kindle.css` for local tuning.

Set:

- `kindle_email` to your `@kindle.com` send-to-kindle address
- `from_email` and `smtp_username` to the Gmail account
- `smtp_password` to a Gmail app password, not the account password

The `from_email` address must be on Amazon's Approved Personal Document E-mail List.

## Native Host Install

Browser native-messaging manifests hard-code the executable path, so use the `cargo install`ed binary (a stable path), not a `target/` build:

```sh
kindlecast install --extension-id CHROME_EXTENSION_ID
```

The installer writes manifests for Google Chrome and Chromium under `~/.config`. For Firefox, pass `--firefox-id` with the extension ID you set as `browser_specific_settings.gecko.id` in the extension manifest (e.g. `--firefox-id kindlecast@example.com`) to also write the Firefox manifest.

Flatpak browsers generally cannot spawn native hosts from the host filesystem; use the RPM/deb browser build for this extension.

## Extension

Load `extension/` as an unpacked Chrome extension, copy its generated extension ID, then run `kindlecast install --extension-id ...`.

The extension enables actions on regular `http(s)` pages. For HN and Lobsters it sends only the URL and lets the native host use the clean JSON APIs. For Reddit, it captures the rendered page DOM because Reddit's public JSON/HTML endpoints commonly return HTTP 403 from unauthenticated clients; the native host parses the visible post and comment tree directly from the captured HTML. For generic articles it also captures the rendered page DOM, which helps on JavaScript-heavy, bot-walled, or logged-in pages; if capture is blocked, the host falls back to fetching the URL directly.

Reddit's captured-DOM extraction only includes comments already visible in the browser; it does not expand "load more" links or fetch additional content. The CLI still depends on Reddit's public JSON API, so Reddit may be unreachable from the command line on networks where the JSON endpoint returns 403. Use the browser extension for reliable Reddit send.

## Notes

HN comment content comes from Algolia (one request for the whole tree), but Algolia's ordering is chronological, so kindlecast fetches the official Firebase API's ranked `kids` arrays for branches with 2+ replies and reorders to match the page. Branches whose lookup fails keep chronological order. Algolia prunes deleted/dead comments, so the rendered count can differ from HN's displayed count.

Reddit public JSON may return 403 from some networks. The browser extension captures the rendered discussion DOM instead, parsing old Reddit (`thing` classes) and current desktop (`shreddit-*` elements) layouts to produce the Kindle discussion. If captured parsing fails, the JSON API is tried as a compatibility fallback. Only comments already rendered in the captured page are included; "load more" placeholders are reported but not expanded. Login, consent, and bot-block pages are rejected rather than silently turned into generic articles. CLI Reddit support remains API-only and will show a clear error when the JSON endpoint is unavailable — use the browser extension as the supported Reddit path.

Rendering goes through pandoc (`html → epub3`, `--split-level=1`), so every `<h1>` becomes a chapter — that's the mechanism behind per-thread chapters. Pandoc's HTML reader drops attributes from `<p>` tags, which is why classed block lines in `render.rs` are `<div>`s.

Kindle depth styling relies on `margin-left`; left borders are progressive enhancement and may be dropped by Enhanced Typesetting. Headings inside comment bodies are neutralized (they'd otherwise fragment chapters), and `# headings` in Reddit selftext are demoted to `<h2>`.

Sites that serve images only in modern formats (JPEG XL / AVIF / WebP, e.g. fasterthanli.me) will have broken images on-device — Kindle only handles JPEG/PNG/GIF/BMP and kindlecast doesn't transcode.
