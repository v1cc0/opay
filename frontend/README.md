# frontend

这里不是一个孤立的“静态前端 demo”，而是 OPay 的实际用户端 / 管理端入口。

它负责：

- 用户充值页 `/pay`
- 用户订单页 `/pay/orders`
- 支付结果页 `/pay/result`
- Stripe popup 页 `/pay/stripe-popup`
- 管理端 `/admin/*`
- 把 `/api/*` 和 `/healthz` 代理到 Rust backend

---

## 1. 这个前端现在能做什么

### 用户侧

- 展示充值与订阅入口
- 创建 Stripe / EasyPay 订单
- 在支付页轮询订单状态
- 查看支付结果页
- 查看“我的订单”
- 对符合条件的订单发起退款申请

### 管理侧

- 查看订单管理页面
- 查看支付配置页面
- 查看渠道管理页面
- 查看订阅管理页面
- 走管理员订单动作相关 UI

### 网关层

- 处理 SPA 路由
- 给静态资产响应补安全响应头
- 把 `/api/*` / `/healthz` 转发到 backend
- 透传 `x-forwarded-host` / `x-forwarded-proto`

---

## 2. 最快上手

### 先安装依赖

```bash
pnpm --dir frontend install
```

### 本地开发前准备 `.dev.vars`

```bash
cp frontend/.dev.vars.example frontend/.dev.vars
```

默认最重要的变量是：

- `API_BASE_URL=http://127.0.0.1:8080`

如果你需要允许 iframe 嵌入，还可以继续加：

- `PLATFORM_BASE_URL`
- `IFRAME_ALLOW_ORIGINS`

### 启动前端 dev

```bash
pnpm --dir frontend cf:dev
```

---

## 3. 和后端联调怎么跑

最常见的联调方式：

### 第一步：启动 backend

在仓库根目录：

```bash
cp config.example.toml config.toml
cargo run
```

### 第二步：启动 frontend worker

```bash
cp frontend/.dev.vars.example frontend/.dev.vars
pnpm --dir frontend install
pnpm --dir frontend cf:dev
```

### 第三步：访问页面

常用入口：

- 用户充值页：`/pay`
- 用户订单页：`/pay/orders`
- 支付结果页：`/pay/result`
- 管理端首页：`/admin`
- 管理端订单：`/admin/orders`
- 管理端支付配置：`/admin/payment-config`

如果要走真实业务态，通常还会带：

- 用户 token：`?token=...`
- 管理员 token：`?token=...`

---

## 4. 常用命令

```bash
pnpm --dir frontend install
pnpm --dir frontend build
pnpm --dir frontend preview
pnpm --dir frontend cf:check
pnpm --dir frontend cf:dev
pnpm --dir frontend cf:deploy
```

说明：

- `build`：本地构建产物
- `cf:check`：构建 + `wrangler deploy --dry-run`
- `cf:dev`：本地 worker dev
- `cf:deploy`：真实 deploy

---

## 5. 这个前端和 backend 的边界

### 前端自己处理的事

- 页面路由
- UI 渲染
- 交互状态
- 结果页轮询
- 管理页表单与列表

### backend 处理的事

- 订单创建
- 支付 provider 调用
- webhook / notify 验签
- 履约
- 退款
- 管理员动作
- DB 读写

### worker 处理的事

- 静态资源服务
- 安全响应头
- API 代理

---

## 6. 重要环境变量

### 必填

- `API_BASE_URL`
  - Rust backend 地址
  - 例如：`http://127.0.0.1:8080`

### 可选

- `PLATFORM_BASE_URL`
  - 用于构造 `frame-ancestors`
- `IFRAME_ALLOW_ORIGINS`
  - 额外允许嵌入的来源，逗号分隔

---

## 7. 路由说明

当前主要路由：

- `/` -> 重定向到 `/pay`
- `/pay`
- `/pay/orders`
- `/pay/result`
- `/pay/stripe-popup`
- `/admin`
- `/admin/dashboard`
- `/admin/orders`
- `/admin/payment-config`
- `/admin/channels`
- `/admin/subscriptions`

---

## 8. 构建与部署注意事项

- 这是一个 SPA，`wrangler.jsonc` 里已经启用了 SPA fallback
- 当前是 client-side routing（`react-router-dom`）
- `/api/*` 和 `/healthz` 都会被 worker 代理到 backend
- 如果 backend 没起来，页面壳子可能能打开，但业务数据会是空的或报错

---

## 9. 更推荐的验证方式

如果你不是只改 UI，而是改支付流程、订单、恢复补偿、管理动作，别只盯着前端 dev。

更推荐直接回到仓库根目录，用统一 smoke suite：

```bash
python3 scripts/run_local_smoke_suite.py
```

那样验证的是整条业务链路，而不只是页面能不能打开。
