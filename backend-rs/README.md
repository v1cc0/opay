# opay-rs

Native Turso backend for OPay.

## Current scope

This is only the first MVP backend skeleton:

- Axum HTTP server
- Native Turso database initialization
- custom SQL migration runner
- health endpoint
- admin config endpoint
- admin env-defaults endpoint
- admin provider instance CRUD
- Platform client skeleton
- user config endpoint skeleton
- order repository / audit repository skeleton
- order service skeleton for create / expire / confirm
- payment provider abstraction skeleton
- basic application state wiring
- Turso schema v1 draft using integer cents for money fields
- TS-compatible provider config encryption format (`iv:authTag:ciphertext`)

## Run

```bash
cd backend-rs
cp config.example.toml config.toml
cargo run
```

Default bind address:

- `0.0.0.0:8080`

Default database path:

- `<repo>/backend-rs/data/opay.db`

## Config file

Rust backend 现在优先读取 `config.toml`，不再把 `.env` 当主配置入口。

默认查找顺序：

1. 当前工作目录 `./config.toml`
2. `backend-rs/config.toml`

推荐流程：

```bash
cd backend-rs
cp config.example.toml config.toml
```

`config.toml` 主要分三块：

- `[app]`：核心运行参数
- `[runtime]`：运行时参数，例如 `rust_log`
- `[env]`：保留给现有 env 风格键值，兼容支付网关密钥、费率覆盖等老逻辑

关键字段：

- `[app].host` default `0.0.0.0`
- `[app].port` default `8080`
- `[app].db_path` default `<repo>/backend-rs/data/opay.db`
- `[app].payment_providers` Rust MVP-A 当前只应填 `["easypay", "stripe"]`
- `[app].admin_token` 管理后台 token
- `[app].platform_base_url` `/api/user` 等接口需要
- `[runtime].rust_log` default `info,tower_http=info`
- `[env]` 可放：
  - `EASY_PAY_*`
  - `STRIPE_*`
  - `ORDER_TIMEOUT_MINUTES`
  - `IFRAME_ALLOW_ORIGINS`
  - `FEE_RATE_*`

## Docker

构建镜像：

```bash
docker build -t opay-rs:latest backend-rs
```

使用仓库根目录的 compose 文件启动：

```bash
IMAGE_TAG=latest docker compose -f docker-compose.rust.yml up -d
```

默认行为：

- 宿主机端口：`RUST_APP_PORT`，默认 `8080`
- 容器内监听：固定 `0.0.0.0:8080`
- 数据目录：`./backend-rs-data -> /data`
- 数据库文件：`/data/opay.db`
- 配置文件挂载：`./backend-rs/config.toml -> /app/config.toml`

## Available routes

- `GET /healthz`
- `POST /api/orders`
- `GET /api/easy-pay/notify`
- `POST /api/stripe/webhook`
- `GET /api/admin/config`
- `PUT /api/admin/config`
- `GET /api/admin/config/env-defaults`
- `GET /api/admin/provider-instances`
- `POST /api/admin/provider-instances`
- `GET /api/admin/provider-instances/{id}`
- `PUT /api/admin/provider-instances/{id}`
- `DELETE /api/admin/provider-instances/{id}`
- `GET /api/user`

## Rust MVP-A scope

- 当前上线范围只包含 `EasyPay + Stripe`
- `alipay_direct / wxpay_direct` 代码仍保留作后续路线参考，但默认不会出现在可用支付方式或 env defaults 中
- 上线步骤参考：`../docs/rust-mvp-a-runbook.md`

## Current payment behavior

`POST /api/orders` now includes a **payment provider skeleton**:

- EasyPay uses a real signed form POST against `/mapi.php`
- EasyPay returns real `tradeNo` / `payUrl` / `qrCode` fields from provider response
- Stripe uses a real PaymentIntent create request against `/v1/payment_intents`
- Stripe returns real `tradeNo` / `clientSecret` fields from provider response
- provider instance selection and fee calculation are already real
- EasyPay notify and Stripe webhook already map back into order confirmation
- balance fulfillment is now wired to Platform `create-and-redeem`
- subscription orders now accept local `subscription_plans` rows and persist `plan_id / subscription_group_id / subscription_days`
- subscription fulfillment is now wired to Platform `create-and-redeem` with `type=subscription`
- active subscription renewals now recompute month-based validity from the current expiry timestamp
- failed balance fulfillment now lands in `FAILED` and can be retried by repeated provider notify/webhook delivery
- failed subscription fulfillment now lands in `FAILED` and can be retried by repeated provider notify/webhook delivery
- refund state machine is now implemented in the Rust service layer:
  - balance orders can enter `REFUND_REQUESTED`
  - admin refund flow supports `COMPLETED / REFUND_REQUESTED / REFUND_FAILED -> REFUNDING`
  - refund success lands in `PARTIALLY_REFUNDED / REFUNDED`
  - gateway failure triggers deduction rollback and state restoration
  - rollback failure lands in `REFUND_FAILED`
- refund HTTP routes are still pending API parity work
