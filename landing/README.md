# Pluk landing page

The marketing page for Pluk — static HTML and CSS in `public/`, no build step, no JavaScript.

## Deploy

From this directory:

```bash
npx wrangler deploy        # or: bunx wrangler deploy
```

Wrangler reads `wrangler.jsonc` and uploads `public/` as a Workers static-assets site.

Nothing in this repo carries account credentials — the person deploying supplies
these from their own Cloudflare account:

- **`CLOUDFLARE_ACCOUNT_ID`** — the account to deploy into. Needed when your
  login or token reaches more than one account, and in CI.
- **`CLOUDFLARE_API_TOKEN`** — a token with the *Workers Scripts: Edit*
  permission. For an interactive machine, `npx wrangler login` once and skip
  the token entirely.

The site lands at `https://pluk-landing.<your-subdomain>.workers.dev`.

## Point pluk.desgn.space at it

Custom domains are attached manually, so the config stays deployable by any
account:

1. Cloudflare dashboard → **Workers & Pages** → `pluk-landing` →
   **Settings → Domains & Routes → Add → Custom domain**.
2. Enter `pluk.desgn.space`. The `desgn.space` zone must be in the same
   Cloudflare account; Cloudflare then creates the DNS record and the
   certificate on its own.

Alternatively, let the deploy attach it by adding this to `wrangler.jsonc`
(and only if that zone is in the deploying account):

```jsonc
"routes": [{ "pattern": "pluk.desgn.space", "custom_domain": true }]
```
