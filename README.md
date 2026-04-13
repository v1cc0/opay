# OPay

`opay` 现在采用新的仓库布局：

- repo root：Rust backend（主骨架）
- `frontend/`：Cloudflare Workers Static Assets frontend

## Layout

```text
.
├── Cargo.toml
├── Cargo.lock
├── Dockerfile
├── config.example.toml
├── migrations/
├── src/              # Rust backend
├── testdata/
├── frontend/         # Vite + React + Wrangler frontend
└── .github/workflows/ci.yml
```

## Backend quick start

```bash
cp config.example.toml config.toml
cargo run
```

## Frontend quick start

```bash
pnpm --dir frontend install
pnpm --dir frontend build
pnpm --dir frontend cf:check
pnpm --dir frontend cf:dev
```

本地联调建议：

1. 先启动 Rust backend（默认 `http://127.0.0.1:8080`）
2. 再启动 Cloudflare Workers frontend dev

## Docker

```bash
docker build -t opay-rs:latest .
IMAGE_TAG=latest docker compose -f docker-compose.rust.yml up -d
```
