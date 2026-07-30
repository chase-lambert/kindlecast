# RustyPub

`rustypub` turns discussion threads and web articles into portable EPUB3 books.
It can save them locally, email them to a reader or service, or do both.

It supports Hacker News, Reddit, Lobsters, and generic `http(s)` articles.

## Why threads read well

- Each top-level comment becomes a chapter, so reader chapter controls move
  between threads.
- Large reply subtrees get skip links.
- Very large discussions are trimmed breadth-first, so you get every top-level
  discussion and the replies under each, rather than a few complete threads and
  nothing else. Whatever is cut is disclosed in the book.
- Indentation stops at a configurable depth instead of consuming the page.
- Remote images are resized and packaged for broad reader compatibility.

On Kindle, Page Flip's chapter arrows move between top-level threads and the
chapter-progress footer measures the current thread.

## Install and use

[Pandoc](https://pandoc.org/installing.html) is required. Then install RustyPub:

```sh
cargo install --path . --force --locked
```

`--force` replaces an existing install in place so the absolute path recorded
for browser native messaging stays valid; `--locked` keeps the build on the
committed `Cargo.lock`.

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

The extension sends the current page to the RustyPub native helper. Chrome,
Chromium, and Firefox share one bundle under `extension/`. Data handling is
described in [PRIVACY.md](PRIVACY.md).

Browser integration is currently documented and tested for **Linux** (same
boundary as Chrome/Chromium native-host paths under `~/.config` and Firefox
under `~/.mozilla/native-messaging-hosts`).

Install the native helper to Cargo's binary directory first:

```sh
cargo install --path . --force --locked
```

The browser registration records that installed helper's absolute path. Prefer
this over registering `target/release/rustypub`, which breaks when the tree is
cleaned or rebuilt elsewhere. When updating the helper, rerun the same
`cargo install` command to replace the binary in place. Re-run
`rustypub install …` only if the registered path changed (for example after
migrating off a `target/` registration); a same-path Cargo reinstall keeps the
existing manifest valid.

Chrome and Firefox cannot share one MV3 `background` block: Chrome rejects
`background.scripts`, and Firefox still requires it (service workers alone are
disabled). Shared scripts live in `extension/`; `manifest.json` is Chrome-ready,
and `manifest.firefox.json` is the Firefox overlay. Use
`scripts/firefox-package.sh` to stage or package Firefox builds.

### Chrome / Chromium

1. Load `extension/` as an unpacked extension and copy its extension ID.
2. Register the native host for the browser you use (from the installed binary):

   ```sh
   rustypub install chrome --extension-id CHROME_EXTENSION_ID
   # or
   rustypub install chromium --extension-id CHROMIUM_EXTENSION_ID
   ```

Use `--dry-run` first to print the native messaging manifest without writing it.
When updating an existing installation, reinstall the helper and reload the
unpacked extension so the browser scripts and native protocol stay in sync.

The installer CLI is browser-specific (breaking change from older
`rustypub install --extension-id … [--firefox-id …]`):

| Old | New |
|-----|-----|
| `install --extension-id ID` | `install chrome --extension-id ID` and/or `install chromium --extension-id ID` |
| `… --firefox-id …` | `install firefox` (add-on ID is fixed) |

### Firefox

Register the native host from the installed binary (the add-on ID is fixed as
`@rustypub.chaselambert`):

```sh
rustypub install firefox
```

For repeated Firefox development, stage an inspectable tree (only under
`target/`, and only replaces a prior stage):

```sh
bash scripts/firefox-package.sh stage
# optional: bash scripts/firefox-package.sh stage target/extension-firefox-wip
```

Then open `about:debugging`, choose “This Firefox”, click “Load Temporary
Add-on”, and select `target/extension-firefox/manifest.json`. Temporary add-ons
are removed when Firefox restarts. The native host registration itself is
durable and survives browser restarts once it points at the Cargo-installed
binary.

For a ZIP (Mozilla lint runs first; staging is disposable):

```sh
bash scripts/firefox-package.sh build
# or: bash scripts/firefox-package.sh lint
```

The package lands in `web-ext-artifacts/`. For a durable extension installation
in standard Firefox, submit that ZIP for unlisted Mozilla signing (after
vacation / when AMO credentials are available), or sign a staged tree:

```sh
bash scripts/firefox-package.sh stage
npx --yes web-ext@10.5.0 sign \
  --source-dir target/extension-firefox \
  --channel unlisted \
  --api-key "$WEB_EXT_API_KEY" \
  --api-secret "$WEB_EXT_API_SECRET"
```

Install the resulting signed `.xpi` from Firefox’s “Install Add-on From File”
command. Signing needs an
[addons.mozilla.org developer account](https://addons.mozilla.org/developers/)
and a privacy-policy URL pointing at [PRIVACY.md](PRIVACY.md).

(`extension/prepare-firefox.sh` remains a thin wrapper around
`scripts/firefox-package.sh stage`.)

### Behavior notes

The extension captures rendered pages when useful, including Reddit and
JavaScript-heavy articles. Capture is skipped for Hacker News and Lobsters
thread URLs. On Reddit, only comments already visible in the page are included;
“load more” links are not expanded. CLI Reddit access can fail when Reddit
returns HTTP 403, so the extension is the more reliable route.

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

- A book holds at most 1,500 comments. The budget is spent one comment at a
  time, cycling through threads in the order the site ranked them and taking
  each thread's replies breadth-first. Every thread gets its opening comment
  before any thread gets a second — up to 1,500 threads, beyond which the
  remainder are left out and counted — and threads that run out release their
  share to the ones that remain, so a lone giant thread beside many small ones
  still gets everything it needs.

  Nothing is hidden. The line under the title reports the extent
  (`1500 of 3767 comments · all 433 threads`), each trimmed chapter states its
  full size (`showing 18 of 436 comments`), and each cut point says what was
  removed (`12 replies omitted`).
- Images are bounded to 100 downloads, 20 MiB each, and 100 MiB total. JPEG,
  PNG, GIF, and WebP are supported; omitted images retain their alt text.
- Requests time out after 30s, under 16 MiB HTML and 32 MiB JSON body budgets.
- Reddit threads are fetched at up to 500 comments. Replies beyond that are
  never fetched, and their total is reported while building rather than in the
  book — unlike budget trimming, which is disclosed inline.
- Hacker News hides comments beneath a flagged or dead parent, and its search
  API omits those subtrees entirely, so they are absent from the book.

## Image fetching and private addresses

Image URLs come out of untrusted page HTML, so the image fetcher resolves
through an address policy: anything that is not publicly routable — loopback,
private ranges, link-local (including `169.254.169.254`), carrier-grade NAT,
unique-local, link-local and site-local IPv6, documentation and discard ranges,
6to4, and IPv4-mapped or IPv4-translated forms of any of those — is refused, and
the image is omitted.
Because the check runs where the address is chosen rather than on the URL, it
also covers redirects.

Two consequences worth knowing:

- **Image fetches ignore `HTTP_PROXY`/`HTTPS_PROXY`.** A proxy would resolve the
  host itself, which would put the target back out of reach of the policy.
  Behind a corporate proxy, article images are omitted rather than fetched
  unchecked.
- This bounds where an image URL can point, not what is hosted there. It is not
  a general network security boundary: an attacker-controlled public address is
  still reachable, as it is for any URL you choose to open. Only the well-known
  NAT64 prefix is recognized — a network running NAT64 from its own address
  space is indistinguishable from ordinary public space here.
