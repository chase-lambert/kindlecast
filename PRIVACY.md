# Privacy

When you open the extension popup, RustyPub reads the active tab's URL locally
so it can enable the action buttons. That URL stays in the popup until you
click Email EPUB or Download EPUB.

## What is sent to the local helper

Only after you click an action does the extension send the current tab's URL to
the RustyPub native helper running on your computer.

For ordinary articles and Reddit threads, the extension may also capture the
page's rendered HTML (`document.documentElement.outerHTML`) and send that to the
same local helper at click time. Capture is skipped for Hacker News and Lobsters
thread URLs, where the helper fetches the discussion itself.

Nothing is sent to RustyPub servers. There are no analytics, advertising,
accounts, or telemetry.

## What leaves your machine

- **Download EPUB** writes a local EPUB file built from the URL and, when
  captured, the page HTML.
- **Email EPUB** sends that EPUB through the mail settings you configured with
  `rustypub init` (your SMTP provider). The attachment is derived reading
  content, not a live browser session.

The helper processes data only for the job you started. It does not keep a
cross-job cache of page HTML on disk beyond the EPUB you chose to save or send.

Third-party sites you open (news sites, Reddit, Hacker News, Lobsters) and any
SMTP provider you configure handle traffic under their own policies.
