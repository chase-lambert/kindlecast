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

## CLI

```sh
cargo run -- 126809 --no-email
cargo run -- 'https://news.ycombinator.com/item?id=126809'
cargo run -- 'https://lobste.rs/s/abc123/title' --no-email
cargo run -- 'https://example.com/article' --no-email --keep-html
```

The default mode saves to `~/Downloads` and emails the EPUB. Use `--no-email` to build only, or `--email-only` to avoid keeping a copy in Downloads.

Pandoc is required:

```sh
sudo apt install pandoc   # or: dnf install pandoc / brew install pandoc
```

## Config

```sh
kindlecast init
```

This writes `~/.config/kindlecast/config.toml` with mode `0600` and copies `kindle.css` for local tuning.

Set:

- `kindle_email` to your `@kindle.com` send-to-kindle address
- `from_email` and `smtp_username` to the Gmail account
- `smtp_password` to a Gmail app password, not the account password

The `from_email` address must be on Amazon's Approved Personal Document E-mail List.

## Native Host Install

Install the binary to a stable path first, because browser native-messaging manifests hard-code the executable path:

```sh
cargo install --path .
kindlecast install --extension-id CHROME_EXTENSION_ID
```

The installer writes manifests for Google Chrome and Chromium under `~/.config`. Pass `--firefox-id kindlecast@chaselambert.dev` to also write the Firefox manifest.

Flatpak browsers generally cannot spawn native hosts from the host filesystem; use the RPM/deb browser build for this extension.

## Extension

Load `extension/` as an unpacked Chrome extension, copy its generated extension ID, then run `kindlecast install --extension-id ...`.

The extension enables actions on regular `http(s)` pages. For HN, Reddit, and Lobsters it sends only the URL and lets the native host use the clean JSON APIs. For generic articles it also captures the rendered page DOM, which helps on JavaScript-heavy, bot-walled, or logged-in pages; if capture is blocked, the host falls back to fetching the URL directly.

## Notes

HN comment content comes from Algolia (one request for the whole tree), but Algolia's ordering is chronological, so kindlecast fetches the official Firebase API's ranked `kids` arrays for branches with 2+ replies and reorders to match the page. Branches whose lookup fails keep chronological order. Algolia prunes deleted/dead comments, so the rendered count can differ from HN's displayed count.

Reddit public JSON may return 403 from some networks. The adapter uses `raw_json=1` and a descriptive user agent, but Reddit still controls access.

Rendering goes through pandoc (`html → epub3`, `--split-level=1`), so every `<h1>` becomes a chapter — that's the mechanism behind per-thread chapters. Pandoc's HTML reader drops attributes from `<p>` tags, which is why classed block lines in `render.rs` are `<div>`s.

Kindle depth styling relies on `margin-left`; left borders are progressive enhancement and may be dropped by Enhanced Typesetting. Headings inside comment bodies are neutralized (they'd otherwise fragment chapters), and `# headings` in Reddit selftext are demoted to `<h2>`.

Sites that serve images only in modern formats (JPEG XL / AVIF / WebP, e.g. fasterthanli.me) will have broken images on-device — Kindle only handles JPEG/PNG/GIF/BMP and kindlecast doesn't transcode.
