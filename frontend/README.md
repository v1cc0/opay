# frontend

Cloudflare **Workers Static Assets** frontend for `opay`.

## Commands

```bash
pnpm --dir frontend dev
pnpm --dir frontend build
pnpm --dir frontend cf:check
pnpm --dir frontend cf:dev
pnpm --dir frontend cf:deploy
```

## Local development

1. Start Rust backend on `http://127.0.0.1:8080`
2. Copy local vars:

```bash
cp frontend/.dev.vars.example frontend/.dev.vars
```

3. Start Worker dev:

```bash
pnpm --dir frontend cf:dev
```

## Important vars

- `API_BASE_URL`: Rust backend origin
- `PLATFORM_BASE_URL`: optional, used to build `frame-ancestors`
- `IFRAME_ALLOW_ORIGINS`: optional extra allowed iframe origins, comma-separated

## Notes

- SPA fallback is configured in `wrangler.jsonc` via `assets.not_found_handling = "single-page-application"`
- Current routing is client-side via `react-router-dom`
- Route-level lazy loading is enabled to avoid one giant bundle
