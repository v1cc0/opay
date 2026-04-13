# opay

`opay` 是从 `sub2apipay` 迁出的新基础盘，当前只保留后续要继续演进的两条主线：

- `backend-rs/`: Rust backend (`axum + turso`)
- `frontend-cf/`: Cloudflare Workers Static Assets frontend

同时迁入了 Workers 前端当前依赖的共享前端源码与静态资源：

- `src/app/admin`
- `src/app/pay`
- `src/app/globals.css`
- `src/components`
- `src/lib`
- `public`

## Quick start

### Rust backend

```bash
cd backend-rs
cp config.example.toml config.toml
cargo run
```

### Workers frontend

```bash
pnpm install
pnpm frontend:build
pnpm frontend:cf:check
```

本地联调：

1. 先启动 Rust backend (`http://127.0.0.1:8080`)
2. 再运行：

```bash
pnpm frontend:cf:dev
```

详细说明见：

- `backend-rs/README.md`
- `frontend-cf/README.md`
