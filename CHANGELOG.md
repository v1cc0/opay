# CHANGELOG

本文档记录 OPay 的对外可感知变更，而不是所有底层提交明细。

格式偏实用：

- 新能力
- 行为变化
- 验证 / 运维相关的重要改进

---

## [v0.1.2] - 2026-04-14

### Added

- 增加可选浏览器 smoke，并接入统一 runner
- 增加 `OPERATIONS.md` 运维文档
- 增加 `RELEASE.md` 发版文档
- 增加 `CHANGELOG.md` 版本变更记录

### Changed

- 根 README 重写为功能说明 + 上手指南导向
- 前端 README 重写为职责边界 + 联调指南导向
- 本地数据库运行时明确区分 readonly 读路径与 concurrent 写路径

### Operational

- 手动触发的 GitHub smoke workflow 可继续用于发版前验证
- 当前发布版本提升到：
  - backend: `0.1.2`
  - frontend: `0.1.2`

---

## [v0.1.1] - 2026-04-14

### Added

- 增加 Stripe 支付完成链路：
  - 创建订单
  - webhook 验签
  - 履约完成
  - 结果页 / 订单页完成态
- 增加 EasyPay 支付完成链路：
  - 创建订单
  - notify 验签
  - 履约完成
  - 结果页 / 订单页完成态
- 增加管理员订单动作验证能力：
  - 查看订单详情
  - 取消待支付订单
  - 重试失败订单
  - 退款处理
- 增加恢复 / 补偿路径覆盖：
  - 支付成功但履约失败后的管理员恢复
  - 退款网关失败后的回滚补偿
  - 退款回滚失败后的人工处理路径
- 增加重复回放幂等验证：
  - Stripe webhook replay
  - EasyPay notify replay
- 增加 MVCC 并发冲突 / 竞争写入 smoke
- 增加统一本地 smoke runner：
  - `scripts/run_local_smoke_suite.py`
- 增加可选浏览器 smoke：
  - Stripe 结果页
  - EasyPay 结果页
  - 用户订单页
  - 管理员订单页
- 增加可选 GitHub Actions 手动 smoke workflow：
  - `.github/workflows/smoke.yml`
- 增加运维 / 发布文档：
  - `OPERATIONS.md`
  - `RELEASE.md`

### Changed

- 本地数据库运行时切到 Turso MVCC：
  - 写路径统一走 `BEGIN CONCURRENT -> COMMIT`
  - 读路径统一走 readonly / `PRAGMA query_only=1`
- README 重写为更偏功能说明和 get started 的风格
- 前端 README 改成用户端 / 管理端职责导向，而不只是命令清单
- 版本号提升到：
  - backend: `0.1.1`
  - frontend: `0.1.1`

### Operational

- 统一 smoke 结果默认输出到：
  - `/tmp/opay-local-smoke-suite.json`
  - `/tmp/opay-local-smoke-logs/`
- 本地 mock 已支持 Stripe / EasyPay 与故障注入控制
- 已知运行注意事项：
  - 本地残留 backend / mock / wrangler 进程可能导致 seed 阶段拿不到 DB 锁

---

## [v0.1.0]

- 初始 Rust backend / frontend 结构落地
- 基础 rebrand 完成
- 前端目录独立收敛到 `frontend/`
- 基础认证链路和缺省页可用
