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
├── examples/         # local smoke helpers
├── migrations/
├── scripts/          # local smoke / mock / recovery runners
├── src/              # Rust backend
├── testdata/
├── frontend/         # Vite + React + Wrangler frontend
└── .github/workflows/
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

## Database runtime model

本地 Turso 运行时现在采用：

- 写路径：`BEGIN CONCURRENT -> COMMIT`
- 读路径：`PRAGMA query_only=1`

也就是说：

- 业务写入统一走 concurrent 事务
- 只读查询统一走 readonly 连接
- 这样能更早暴露误写，也能减少一些锁争用问题

## Local smoke suite

当前 repo 已经内置一套本地 API smoke suite，覆盖：

- Stripe webhook 完成链路
- EasyPay notify 完成链路
- Stripe / EasyPay 重复回放幂等
- 支付成功但履约失败后的管理员恢复
- 退款网关失败后的回滚补偿
- 退款回滚失败后的人工处理路径
- 管理员订单动作 smoke
- MVCC 并发冲突 / 竞争写入 smoke

统一入口：

```bash
python3 scripts/run_local_smoke_suite.py
```

默认输出：

- 汇总结果：`/tmp/opay-local-smoke-suite.json`
- 阶段日志：`/tmp/opay-local-smoke-logs/`

### 运行前注意

在本地手动跑 suite 之前，最好先确保没有残留的 smoke 相关进程，否则 seed 阶段可能因为 DB 文件锁失败：

```bash
pkill -f '/home/vc/.cargo/target/debug/opay|scripts/local_smoke_mocks.py|wrangler dev' || true
```

### 常用单项脚本

```bash
python3 scripts/configure_local_stripe_mock.py
python3 scripts/configure_local_easypay_mock.py
python3 scripts/stripe_webhook_completion_smoke.py
python3 scripts/easypay_notify_completion_smoke.py
python3 scripts/stripe_recovery_smoke.py
python3 scripts/stripe_refund_recovery_smoke.py
python3 scripts/stripe_refund_manual_recovery_smoke.py
python3 scripts/admin_order_actions_smoke.py --seed-only
python3 scripts/admin_order_actions_smoke.py --skip-seed --ids-file /tmp/opay-admin-order-actions-ids.json
cargo run --quiet --example concurrent_write_smoke
```

## Optional CI smoke workflow

除了默认 `ci.yml`，现在还提供一个**手动触发**的 smoke workflow：

- 文件：`.github/workflows/smoke.yml`
- 触发方式：GitHub Actions -> `Smoke Suite` -> `Run workflow`

这个 workflow 会：

1. 生成本地 smoke `config.toml`
2. 运行 `scripts/run_local_smoke_suite.py`
3. 上传 artifacts：
   - `opay-local-smoke-summary`
   - `opay-local-smoke-logs`

这样不会污染普通 push / PR 的常规 CI，但在需要时可以完整跑一次本地化 smoke。
