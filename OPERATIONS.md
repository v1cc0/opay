# OPERATIONS

这份文档偏运行和排障，不偏开发细节。

---

## 1. 服务分层

### backend

- Rust 服务
- 管订单、支付、履约、退款、恢复

### frontend

- Cloudflare Worker 前端入口
- 提供页面和 `/api/*` 代理

### local mocks

本地 smoke 使用的 mock：

- Platform mock
- Payment provider mock（Stripe + EasyPay）

---

## 2. 数据库运行方式

当前本地 Turso 运行时已经明确分成：

- **写路径**：`BEGIN CONCURRENT -> COMMIT`
- **读路径**：readonly + `PRAGMA query_only=1`

这意味着：

- 误把读操作拿去写，会更早暴露
- 生产读查询默认不占写能力
- 锁争用和“谁都能写”的混乱会少很多

---

## 3. 最常用入口

### 启动 backend

```bash
cp config.example.toml config.toml
cargo run
```

### 启动 frontend

```bash
cp frontend/.dev.vars.example frontend/.dev.vars
pnpm --dir frontend install
pnpm --dir frontend cf:dev
```

### 跑统一 smoke

```bash
python3 scripts/run_local_smoke_suite.py
```

### 跑带浏览器的 smoke

```bash
python3 scripts/run_local_smoke_suite.py \
  --with-browser \
  --browser-runner-cmd "node /home/vc/.codex/skills/playwright-skill/run.js"
```

---

## 4. 当前 smoke 覆盖范围

### 支付成功链路

- Stripe webhook 完成
- EasyPay notify 完成

### 恢复 / 补偿

- Stripe 履约失败后的管理员重试恢复
- Stripe 退款网关失败后的回滚补偿
- Stripe 退款回滚失败后的人工处理

### 管理动作

- 订单详情
- 取消
- 重试
- 退款

### 稳定性

- 重复 webhook / notify 幂等回放
- MVCC 并发冲突
- 多 writer 竞争写入

### 浏览器 smoke（可选）

- Stripe 结果页
- EasyPay 结果页
- 用户订单页
- 管理员订单页

---

## 5. 最重要的产物

统一 suite 默认输出：

- 汇总：`/tmp/opay-local-smoke-suite.json`
- 日志：`/tmp/opay-local-smoke-logs/`

浏览器模式还会在日志目录里产生：

- `browser/stripe-result.png`
- `browser/easypay-result.png`
- `browser/user-orders.png`
- `browser/admin-orders.png`
- `browser_smoke.log`

---

## 6. 常见问题

### seed 阶段报 DB 锁

优先处理残留进程：

```bash
pkill -f '/home/vc/.cargo/target/debug/opay|scripts/local_smoke_mocks.py|wrangler dev' || true
```

### backend 没问题，但页面 smoke 跑不起来

优先检查：

- `frontend/.dev.vars`
- frontend worker 是否真的起在 `8787`
- `--browser-runner-cmd` 是否可用
- `/tmp/opay-local-smoke-logs/browser_smoke.log`

### 退款失败相关排障

现在要先分清是哪一类：

1. 网关退款失败，但回滚成功
2. 网关退款失败，回滚也失败
3. 人工处理后再次成功退款

这些路径现在都已经有对应 smoke。

---

## 7. 什么时候优先用哪种验证

### 只改后端纯逻辑

```bash
cargo test --locked
```

### 改支付 / 订单 / 履约 / 退款 / 恢复

```bash
python3 scripts/run_local_smoke_suite.py
```

### 改真实页面交互

```bash
python3 scripts/run_local_smoke_suite.py \
  --with-browser \
  --browser-runner-cmd "node /home/vc/.codex/skills/playwright-skill/run.js"
```
