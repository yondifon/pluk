# Wande

The Chrome extension Pluk drives X through. It pairs to the `/wande` routes on
Pluk's embedded server over a WebSocket, picks up one browser job at a time,
drives the page in an ordinary signed-in Chrome window, and posts the result
back.

Posting is one job, not two. A post or reply an agent asks for waits in Pluk
until the user sends it, queues it, or discards it; the extension is never
told about it before then. Sending — now, or when a queued slot comes due —
dispatches a single `submit_post` or `submit_reply` command that opens the
composer, types the confirmed text, and submits.

## Build

```bash
bun install --cwd extension
bun run --cwd extension build
```

That writes an unpacked extension to `extension/dist`. Load it in Chrome via
`chrome://extensions` → Developer mode → **Load unpacked** → `extension/dist`.

## Pair it

Add a Wande integration in the Pluk app, copy the Pluk ID from its Overview,
paste it into Wande's options page, then grant site access for X. That is the
whole setup — the address is not typed.

The extension finds the server itself: it probes `GET /wande/healthz` (the one
route that needs no credential) on `127.0.0.1:4242` and the eight ports above
it, and keeps whichever answers. An address field behind **Pluk runs on another
port** covers a server outside that range.

The Pluk ID is minted on first start and kept in Pluk's own store. Override it
with an environment variable:

```bash
PLUK_BROWSER_TOKEN=<16-256 chars, no spaces> cargo run -p pluk-host
```

Or read the one Pluk already minted:

```bash
sqlite3 ~/.pluk/pluk.db "select value from settings where key = 'browser_pairing_token'"
```

## Layout

- `src/connection.ts` — the WebSocket transport, server discovery, the command ledger, artifact upload
- `src/browser-executor.ts` — owns the automation window and tab, and the navigation gate
- `src/drivers/x.ts` — the X driver: reading, and typing-and-submitting a confirmed post
- `src/protocol.ts` — the TypeScript mirror of `crates/pluk-browser/src/protocol.rs`

Two details in the X driver are load-bearing and easy to break:

- Text reaches the composer through a **single paste event**.
  `document.execCommand("insertText")` double-applies on X's DraftJS editor and
  duplicates every post.
- Submission sends **Cmd+Enter**, and falls back to clicking
  `[data-testid="tweetButton"]` only when the composer still holds the exact
  text, no new toast appeared, and the button is still enabled.
