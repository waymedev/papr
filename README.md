<div align="center">

<img src="docs/logo.svg" alt="Papr" width="96" height="96" />

# Papr

A fast, native RSS reader for the desktop.

<img src="docs/screenshot.png" alt="Papr" width="820" />

</div>

## Features

- **Feeds & folders** — subscribe, organize, and import/export OPML.
- **Smart views** — All, Unread, Starred, and Read Later, with live counts.
- **Tags & rules** — color-coded tags and rules that tag new articles automatically.
- **Full-text** — fetch and clean the complete article when a feed ships only a summary.
- **AI** — summaries, ask-the-article Q&A, and digests. Bring your own API key.
- **Audio** — a built-in player that follows you from article to article.
- **FreshRSS sync** — keep read state in step with a FreshRSS server.
- **Local-first** — everything in a local SQLite database. No account, no cloud.
- **Localized** — English, Japanese, and Simplified Chinese.

## Installation

Grab the installer for your platform from the [latest release](https://github.com/l0ng-ai/papr/releases/latest).

### macOS

The macOS build is not yet signed or notarized with an Apple Developer
certificate, so Gatekeeper will report **“Papr is damaged and can't be
opened”**. The app is not actually damaged — macOS just refuses to run
unsigned apps that still carry the download quarantine flag.

After dragging Papr into `/Applications`, clear the flag once:

```bash
xattr -cr /Applications/Papr.app
```

Then open it normally. Signed, notarized builds are planned.
