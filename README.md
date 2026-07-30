# RustyPub

`rustypub` turns discussion threads and web articles into portable EPUB3 books.
It saves them locally and can email them as attachments to any reader or service
with an ingest address.

Supported sources:

- Hacker News item URLs and bare HN IDs
- Reddit comment threads (`reddit.com/comments/...`, `redd.it/...`, and share links)
- Lobsters stories
- Generic `http(s)` articles through Readability-style extraction

## Built for reading big threads on e-ink

E-readers cannot reproduce a browser's collapsible comment tree, so RustyPub
translates that interaction into book structure:

- **One chapter per top-level comment.** Every top-level comment becomes its own
  chapter, titled `author · first words of the comment…` in the table of
  contents. Reader chapter controls move thread-to-thread.
- **Skip links.** Any comment with 5+ replies beneath it gets a small
  `skip N replies ↓` link that jumps just past the subtree.
- **Depth-capped indentation.** Nesting indents up to 5 levels (configurable via
  `max_indent_depth`); deeper comments stay at the cap with a `↳ depth` marker
  so walls of margin never consume the page.
- **E-reader-sized images.** Remote JPEG, PNG, GIF, and WebP images are localized
  before packaging, resized to a 1600-pixel longest edge, and emitted as JPEG or
  PNG. One unavailable or unsupported image becomes a short alt-text marker
  instead of failing the article.

On Kindle, Page Flip's chapter arrows move between top-level threads, the full
table of contents is available from the top-of-screen menu, and the "chapter
progress" footer measures the current thread.

Image work is bounded to 100 distinct remote fetches, 20 MiB per response,
100 MiB total input, 25 megapixels and 128 MiB decoded per image, with a
30-second timeout per request. Images beyond those limits are omitted with the
same alt-text marker.

Comment ordering matches the site: for HN, ranked order comes from the official
Firebase API (see Notes).

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
rustypub 126809 --no-email
rustypub 'https://news.ycombinator.com/item?id=126809'
rustypub 'https://lobste.rs/s/abc123/title' --no-email
rustypub 'https://example.com/article' --no-email --keep-html
```

The default mode saves to `~/Downloads` and emails the EPUB. Use `--no-email`
to build only, or `--email-only` to avoid keeping the ordinary output copy.

RustyPub uses a conservative 20 MiB attachment budget and never attempts SMTP
for a larger EPUB. In the default mode, the completed book remains in the output
directory for manual import. For an oversized `--email-only` build, RustyPub
preserves a recovery copy in the configured output directory before returning
the size error. Dropbox's Email to Dropbox service also limits total attachments
to 20 MB (decimal), which is slightly smaller than RustyPub's 20 MiB budget;
Kobo/Dropbox users should keep EPUBs below 20 MB.

## Config

```sh
rustypub init
```

This writes `~/.config/rustypub/config.toml` with permissions `0600` and copies
`reader.css` for local tuning.

For email delivery, set:

- `device_email` to the attachment-ingest address for your reader or service
- `from_email` and `smtp_username` to the sending account
- `smtp_password` to that account's app password
- `smtp_host` if you use a relay other than Gmail

Email settings may be omitted from the file when RustyPub is used only with
`--no-email`. Output and indentation settings remain usable independently.

## Getting the EPUB onto a reader

These are vendor-documented ingestion paths, not hands-on certification across
every reader model and firmware version. RustyPub itself always produces the same
passive EPUB3 and, when requested, sends the same SMTP attachment.

| Reader or service | `device_email` | Mechanism and constraints |
| --- | --- | --- |
| Kindle | Your `@kindle.com` Send to Kindle address | Amazon accepts EPUB through Send to Kindle. Add `from_email` to the Approved Personal Document E-mail List. |
| Supported Kobo models | Your unique Email to Dropbox address | Requires a paid Dropbox plan and a Kobo model with Dropbox integration. Link Dropbox on the reader, then open the emailed EPUB from Dropbox. |
| Apple Books | Leave unset and use `--no-email` | Import the saved EPUB into Books on a Mac; iCloud for Books can make imported books available on other Apple devices. |
| Other EPUB readers | A documented attachment-ingest address, if offered | Otherwise use `--no-email` and import or sideload the saved EPUB through the reader's library or desktop app. |

Vendor documentation:

- [Amazon Send to Kindle](https://www.amazon.com/sendtokindle)
- [Kobo: email books through Dropbox](https://www.kobo.com/blog/how-to-email-books-to-your-kobo)
- [Kobo: supported readers and Dropbox setup](https://help.kobo.com/hc/en-us/articles/360033830114-Add-books-to-your-eReader-using-Dropbox)
- [Dropbox: Email to Dropbox requirements and limits](https://help.dropbox.com/create-upload/email-files-to-dropbox)
- [Apple Books: import EPUBs](https://support.apple.com/en-hk/guide/books/ibkseed72068/8.0/mac/26)

## Browser extension

Native-messaging manifests hard-code the executable path, so complete setup in
this order:

1. Install the binary with `cargo install --path .`.
2. Load `extension/` as an unpacked Chrome extension and copy its generated
   extension ID.
3. Register the installed binary:

   ```sh
   rustypub install --extension-id CHROME_EXTENSION_ID
   ```

The installer writes manifests for Google Chrome and Chromium under `~/.config`.
For Firefox, pass `--firefox-id` with the extension ID set as
`browser_specific_settings.gecko.id` in the extension manifest:

```sh
rustypub install \
  --extension-id CHROME_EXTENSION_ID \
  --firefox-id rustypub@example.com
```

Flatpak browsers generally cannot spawn native hosts from the host filesystem;
use the RPM/deb browser build for this extension.

The extension enables actions on regular `http(s)` pages. For HN and Lobsters it
sends only the URL and lets the native host use the clean JSON APIs. For Reddit,
it captures the rendered page DOM because Reddit's public JSON/HTML endpoints
commonly return HTTP 403 from unauthenticated clients; the native host parses the
visible post and comment tree directly. For generic articles it also captures
the rendered page DOM, which helps on JavaScript-heavy, bot-walled, or logged-in
pages; if capture is blocked, the host falls back to fetching the URL directly.

Reddit's captured-DOM extraction only includes comments already visible in the
browser; it does not expand "load more" links or fetch additional content. The
CLI still depends on Reddit's public JSON API, so Reddit may be unreachable from
the command line on networks where the JSON endpoint returns 403. Use the browser
extension for reliable Reddit delivery.

## Notes

HN comment content comes from Algolia (one request for the whole tree), but
Algolia's ordering is chronological, so RustyPub fetches the official Firebase
API's ranked `kids` arrays for branches with 2+ replies and reorders to match the
page. Branches whose lookup fails keep chronological order. Algolia prunes
deleted/dead comments, so the rendered count can differ from HN's displayed
count.

Reddit public JSON may return 403 from some networks. The browser extension
captures the rendered discussion DOM instead, parsing old Reddit (`thing`
classes) and current desktop (`shreddit-*` elements) layouts to produce the
discussion book. If captured parsing fails, the JSON API is tried as a
compatibility fallback. Only comments already rendered in the captured page are
included; "load more" placeholders are reported but not expanded. Login,
consent, and bot-block pages are rejected rather than silently turned into
generic articles. CLI Reddit support remains API-only and will show a clear
error when the JSON endpoint is unavailable.

Rendering goes through pandoc (`html → epub3`, `--split-level=1`), so every
`<h1>` becomes a chapter—the mechanism behind per-thread chapters. Pandoc's HTML
reader drops attributes from `<p>` tags, which is why classed block lines in
`render.rs` are `<div>`s.

Depth styling relies on `margin-left`; left borders are progressive enhancement
and may be dropped by Kindle Enhanced Typesetting. Headings inside comment
bodies are neutralized because they would otherwise fragment chapters, and
`# headings` in Reddit selftext are demoted to `<h2>`.

RustyPub decodes JPEG, PNG, GIF, and WebP input and packages images as JPEG or
PNG for broad reader compatibility. AVIF, JPEG XL, remote SVG, unknown formats,
and images that exceed the download or decoded-pixel budgets are omitted with
their alt text preserved as reading context. Inline SVG is removed before
packaging; unlabeled decorative vectors disappear without an omission marker.

Extracted pages are packaged as passive reading documents. Inline event handlers
and `javascript:`, `vbscript:`, or `data:` link targets are removed while their
visible text remains.
