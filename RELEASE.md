# RELEASE

这份文档是发版清单，不是开发说明。

---

## 1. 发版前检查

至少过这些：

```bash
cargo test --locked
pnpm --dir frontend build
python3 scripts/run_local_smoke_suite.py
```

如果你要把页面层也顺手验掉：

```bash
python3 scripts/run_local_smoke_suite.py \
  --with-browser \
  --browser-runner-cmd "node /home/vc/.codex/skills/playwright-skill/run.js"
```

---

## 2. 版本号同步位置

发版时同步：

- `Cargo.toml`
- `Cargo.lock`
- `frontend/package.json`
- 根 `README.md`（如果版本文案有变化）

---

## 3. 提交 / 推送 / Release

### 提交

```bash
git add Cargo.toml Cargo.lock frontend/package.json README.md
git commit -m "Prepare vX.Y.Z release docs"
```

### 推送

```bash
git push origin main
```

### 创建 GitHub Release

```bash
gh release create vX.Y.Z --title "vX.Y.Z" --generate-notes
```

---

## 4. GitHub Actions

仓库现在有两类 workflow：

### 常规 CI

- `.github/workflows/ci.yml`

### 手动 smoke

- `.github/workflows/smoke.yml`

手动 smoke 适合发版前点一次，产出：

- `opay-local-smoke-summary`
- `opay-local-smoke-logs`

---

## 5. 推荐发版顺序

1. 跑本地测试
2. 跑本地 smoke suite
3. 更新版本号
4. 更新 README / 文档
5. commit
6. push
7. 手动触发 GitHub `Smoke Suite`（推荐）
8. 创建 GitHub Release

---

## 6. 当前最近版本

- `v0.1.1`
