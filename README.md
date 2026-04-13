# OPay

OPay 是一个面向“余额充值 / 订阅购买 / 订单管理 / 退款处理”的支付网关。

它的目标不是做一个花哨的支付展示页，而是把下面这些真实业务动作串成一套可运行、可验证、可恢复的流程：

- 用户创建充值订单
- 用户创建订阅订单
- 对接多支付通道（当前重点是 Stripe / EasyPay）
- 支付成功后的 webhook / notify 履约
- 管理员查看订单详情、取消、重试、退款
- 失败后的恢复、补偿与人工处理
- 本地可重复运行的 smoke / recovery / concurrency 验证

---

## 1. 现在这个仓库能做什么

### 用户侧

- 展示充值与订阅入口
- 创建余额充值订单
- 创建订阅订单
- 查询“我的订单”
- 在支付页轮询订单状态
- 在结果页查看支付结果
- 对满足条件的订单发起退款申请

### 管理员侧

- 查看订单列表与订单详情
- 取消待支付订单
- 对失败订单执行重试充值
- 对已完成订单执行退款
- 处理退款网关失败、回滚失败、人工恢复等路径
- 配置支付 provider instance 与系统配置项

### 支付与履约

- Stripe：创建支付、接 webhook、完成履约
- EasyPay：创建支付、接 notify、完成履约
- 支持重复回放幂等验证
- 支持支付成功但履约失败后的恢复
- 支持退款失败后的回滚补偿与人工处理

### 运行稳定性

- 本地 Turso 已切到 `mvcc`
- 写路径统一走 `BEGIN CONCURRENT -> COMMIT`
- 读路径统一走 readonly / `PRAGMA query_only=1`
- 已有并发冲突 / 竞争写入 smoke

---

## 2. 仓库结构

```text
.
├── Cargo.toml
├── Cargo.lock
├── config.example.toml
├── migrations/
├── src/                      # Rust backend
├── frontend/                 # 用户/管理端前端
├── scripts/                  # smoke / mock / recovery 脚本
├── examples/                 # 辅助 smoke / seed / concurrency example
└── .github/workflows/        # CI 与可选 smoke workflow
```

---

## 3. 最快上手：只跑后端

如果你现在只是想把后端启动起来：

```bash
cp config.example.toml config.toml
cargo run
```

默认健康检查：

```bash
curl http://127.0.0.1:8080/healthz
```

---

## 4. 最快上手：本地联调前后端

### 第一步：准备后端配置

```bash
cp config.example.toml config.toml
```

如果你是按本仓库当前 smoke 习惯跑本地开发，至少要关注这些配置：

- `db_path`
- `admin_token`
- `platform_base_url`
- `PLATFORM_ADMIN_API_KEY`

### 第二步：启动后端

```bash
cargo run
```

### 第三步：准备前端环境变量

```bash
cp frontend/.dev.vars.example frontend/.dev.vars
```

### 第四步：启动前端 worker

```bash
pnpm --dir frontend install
pnpm --dir frontend cf:dev
```

然后访问：

- 用户端：`http://127.0.0.1:8787/pay?...`
- 管理端：`http://127.0.0.1:8787/admin/...`

---

## 5. 最推荐的本地验证方式：统一 smoke suite

这个仓库已经不是“靠人手点一点页面碰碰运气”的状态了。

现在最推荐的本地验证入口是：

```bash
python3 scripts/run_local_smoke_suite.py
```

它会自动完成这些事：

1. 启动本地 mock 服务
2. 启动 backend
3. 配置 Stripe / EasyPay 本地 provider instance
4. 跑 Stripe 完成链路
5. 跑 Stripe 履约恢复链路
6. 跑 Stripe 退款回滚补偿链路
7. 跑 Stripe 退款人工处理链路
8. 跑 EasyPay 完成链路
9. 跑管理员订单动作 smoke
10. 跑并发冲突 / 竞争写入 smoke
11. 自动收尾并输出结果

默认输出：

- 汇总：`/tmp/opay-local-smoke-suite.json`
- 日志目录：`/tmp/opay-local-smoke-logs/`

---

## 6. 常用单项脚本

如果你不想整套跑，只想验证某一段链路：

```bash
python3 scripts/configure_local_stripe_mock.py
python3 scripts/configure_local_easypay_mock.py

python3 scripts/stripe_webhook_completion_smoke.py
python3 scripts/stripe_recovery_smoke.py
python3 scripts/stripe_refund_recovery_smoke.py
python3 scripts/stripe_refund_manual_recovery_smoke.py

python3 scripts/easypay_notify_completion_smoke.py

python3 scripts/admin_order_actions_smoke.py --seed-only
python3 scripts/admin_order_actions_smoke.py --skip-seed --ids-file /tmp/opay-admin-order-actions-ids.json

cargo run --quiet --example concurrent_write_smoke
```

---

## 7. 本地运行注意事项

### 先清理残留进程

如果你手上残留了旧的 backend / mock / wrangler 进程，再跑统一 smoke suite，可能会直接因为 DB 文件锁或端口冲突炸掉。

建议在手动跑 suite 前先清理：

```bash
pkill -f '/home/vc/.cargo/target/debug/opay|scripts/local_smoke_mocks.py|wrangler dev' || true
```

### 关于本地数据库

当前本地 Turso 已经不是“随便拿 sqlite3 打开就能直接改”的模式。

现在采用的是：

- 写：MVCC + concurrent transaction
- 读：readonly / `query_only=1`

所以：

- 不要再用标准 `sqlite3` / Python `sqlite3` 直接去改正在被 backend 使用的 DB 文件
- 要 seed / smoke / 恢复，优先复用仓库内脚本和 example

---

## 8. CI 怎么跑

默认 CI 在：

- `.github/workflows/ci.yml`

它负责基础构建和测试。

另外还有一个**手动触发**的 smoke workflow：

- `.github/workflows/smoke.yml`

触发方式：

1. 打开 GitHub Actions
2. 选择 `Smoke Suite`
3. 点击 `Run workflow`

这个 workflow 会：

- 生成本地 smoke `config.toml`
- 运行 `scripts/run_local_smoke_suite.py`
- 上传 artifacts：
  - `opay-local-smoke-summary`
  - `opay-local-smoke-logs`

好处是：

- 不污染普通 push / PR 的常规 CI
- 需要的时候可以完整跑一次本地化 smoke

---

## 9. 当前更适合继续做什么

如果你要在这个仓库上继续推进，优先级建议是：

1. 把浏览器 smoke 逐步并入统一 runner
2. 把 README / 开发文档继续补成更完整的 operator guide
3. 视需要把 smoke suite 进一步产品化（例如更细粒度的 job / artifact / release checklist）

---

## 10. 版本

当前发布版本：`v0.1.1`
