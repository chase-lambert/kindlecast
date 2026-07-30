# RustyPub

`rustypub` turns discussion threads and web articles into portable EPUB3 books.
It can save them locally, email them to a reader or service, or do both.

It supports Hacker News, Reddit, Lobsters, and generic `http(s)` articles.

## Why threads read well

- Each top-level comment becomes a chapter, so reader chapter controls move
  between threads.
- Large reply subtrees get skip links.
- Indentation stops at a configurable depth instead of consuming the page.
- Remote images are resized and packaged for broad reader compatibility.

On Kindle, Page Flip's chapter arrows move between top-level threads and the
chapter-progress footer measures the current thread.

## Install and use

[Pandoc](https://pandoc.org/installing.html) is required. Then install RustyPub:

```sh
cargo install --path .
```

Build a book from an HN ID or a supported URL:

```sh
rustypub 126809 --no-email
rustypub 'https://news.ycombinator.com/item?id=126809'
rustypub 'https://lobste.rs/s/abc123/title' --no-email
rustypub 'https://example.com/article' --no-email
```

By default, RustyPub saves the EPUB to `~/Downloads` and emails it. Use
`--no-email` to build only, `--email-only` to send without keeping the ordinary
output copy, or `--keep-html` to retain the intermediate HTML.

## Configure delivery

```sh
rustypub init
```

This creates `~/.config/rustypub/config.toml` and a customizable `reader.css`.
For email delivery, set `device_email`, `from_email`, `smtp_username`, and
`smtp_password`; set `smtp_host` if you do not use Gmail. Email settings are
optional when using `--no-email`.

| Reader | Delivery |
| --- | --- |
| Kindle | Set `device_email` to your `@kindle.com` address and approve `from_email` in [Send to Kindle](https://www.amazon.com/sendtokindle). |
| Supported Kobo models | Set it to your [Email to Dropbox address](https://www.kobo.com/blog/how-to-email-books-to-your-kobo). This requires a paid Dropbox plan and a [Kobo with Dropbox support](https://help.kobo.com/hc/en-us/articles/360033830114-Add-books-to-your-eReader-using-Dropbox). |
| Apple Books | Use `--no-email`, then [import the saved EPUB](https://support.apple.com/en-hk/guide/books/ibkseed72068/8.0/mac/26). |
| Other EPUB readers | Use a documented attachment-ingest address, or `--no-email` and sideload the EPUB. |

RustyPub will not email an EPUB over 20 MiB. Email to Dropbox has a slightly
smaller [20 MB attachment limit](https://help.dropbox.com/create-upload/email-files-to-dropbox).

## Browser extension

The extension sends the current page to RustyPub. Install it in this order:

1. Run `cargo install --path .`.
2. Load `extension/` as an unpacked Chrome extension and copy its extension ID.
3. Register the native host:

   ```sh
   rustypub install --extension-id CHROME_EXTENSION_ID
   ```

For Firefox, also pass `--firefox-id rustypub@example.com`, matching
`browser_specific_settings.gecko.id` in the extension manifest.

The extension captures rendered pages when useful, including Reddit and
JavaScript-heavy articles. On Reddit, only comments already visible in the page
are included; “load more” links are not expanded. CLI Reddit access can fail
when Reddit returns HTTP 403, so the extension is the more reliable route.

Flatpak browsers generally cannot launch the native host; use the RPM/deb browser
build for the extension.

## Reading policy

Extracted pages become passive reading documents. Every fragment of page HTML —
comment bodies, selftext, article bodies — passes one sanitizer before it can
become part of a book:

- A positive allowlist decides what survives. Scripts, styles, embeds, frames,
  form controls, and inline SVG are dropped; unrecognized tags are unwrapped so
  their words remain.
- Event handlers and `javascript:`/`data:` link targets are removed, keeping the
  visible link text.
- Inline `style` attributes and page classes are dropped, so `reader.css` alone
  decides how a book looks.
- Article footnote and section anchors are preserved but namespaced, so they
  keep working without colliding with chapter and skip-link navigation.
- Fragments are sanitized in isolation, so malformed markup in one comment
  cannot restructure the rest of the book. If a book's chapter or skip-link
  structure is inconsistent anyway, the build is refused rather than shipped.

## Limits

- Images are bounded to 100 downloads, 20 MiB each, and 100 MiB total. JPEG,
  PNG, GIF, and WebP are supported; omitted images retain their alt text.
- Requests time out after 30s, under 16 MiB HTML and 32 MiB JSON body budgets.
- Reddit threads are fetched at up to 500 comments; omitted reply counts are
  shown inline.
