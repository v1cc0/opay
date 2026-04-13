# frontend-cf

Cloudflare **Workers Static Assets** frontend for `opay`.

## Current direction

- Static frontend served by Cloudflare Workers Assets
- Edge Worker proxies `/api/*` and `/healthz` to the Rust backend
- Existing client pages/components are reused from the repo `src/` tree through Vite aliases and small Next.js shims
- Rust backend remains the source of truth for business APIs

## Commands

```bash
pnpm --dir frontend-cf dev
pnpm --dir frontend-cf build
pnpm --dir frontend-cf cf:check
pnpm --dir frontend-cf cf:dev
pnpm --dir frontend-cf cf:deploy
```

## Local development

1. Start Rust backend on `http://127.0.0.1:8080`
2. Copy local vars:

```bash
cp frontend-cf/.dev.vars.example frontend-cf/.dev.vars
```

3. Start Worker dev:

```bash
pnpm --dir frontend-cf cf:dev
```

## Important vars

- `API_BASE_URL`: Rust backend origin
- `SUB2API_BASE_URL`: optional, used to build `frame-ancestors`
- `IFRAME_ALLOW_ORIGINS`: optional extra allowed iframe origins, comma-separated

## Notes

- SPA fallback is configured in `wrangler.jsonc` via `assets.not_found_handling = "single-page-application"`
- Current routing is client-side via `react-router-dom`
- Route-level lazy loading is enabled to avoid one giant bundle
