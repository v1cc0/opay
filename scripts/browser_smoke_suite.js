const fs = require('fs');
const path = require('path');
const { chromium } = require('playwright');

function expect(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function ensureText(page, text) {
  await page.getByText(text).waitFor({ timeout: 15000 });
}

async function saveShot(page, outputDir, name) {
  const file = path.join(outputDir, name);
  await page.screenshot({ path: file, fullPage: true });
  return file;
}

(async () => {
  const summaryPath = process.env.OPAY_SMOKE_SUMMARY_PATH;
  const resultPath = process.env.OPAY_BROWSER_RESULT_PATH;
  const outputDir = process.env.OPAY_BROWSER_OUTPUT_DIR;
  const baseUrl = process.env.OPAY_BROWSER_BASE_URL || 'http://127.0.0.1:8787';
  const headless = process.env.OPAY_BROWSER_HEADLESS === '1';

  expect(summaryPath, 'OPAY_SMOKE_SUMMARY_PATH is required');
  expect(resultPath, 'OPAY_BROWSER_RESULT_PATH is required');
  expect(outputDir, 'OPAY_BROWSER_OUTPUT_DIR is required');

  const summary = JSON.parse(fs.readFileSync(summaryPath, 'utf8'));
  fs.mkdirSync(outputDir, { recursive: true });

  const browser = await chromium.launch({ headless });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1200 } });

  const screenshots = {};

  const stripe = summary.stripeWebhookCompletion;
  const stripeResultUrl = `${baseUrl}/pay/result?order_id=${encodeURIComponent(stripe.orderId)}&access_token=${encodeURIComponent(stripe.statusAccessToken)}`;
  await page.goto(stripeResultUrl, { waitUntil: 'networkidle', timeout: 30000 });
  await ensureText(page, '充值成功');
  screenshots.stripeResult = await saveShot(page, outputDir, 'stripe-result.png');

  const easyPay = summary.easyPayNotifyCompletion;
  const easyPayResultUrl = `${baseUrl}/pay/result?order_id=${encodeURIComponent(easyPay.orderId)}&access_token=${encodeURIComponent(easyPay.statusAccessToken)}`;
  await page.goto(easyPayResultUrl, { waitUntil: 'networkidle', timeout: 30000 });
  await ensureText(page, '充值成功');
  screenshots.easyPayResult = await saveShot(page, outputDir, 'easypay-result.png');

  await page.goto(`${baseUrl}/pay/orders?token=user-token`, { waitUntil: 'networkidle', timeout: 30000 });
  await page.getByRole('heading', { name: '我的订单' }).waitFor({ timeout: 15000 });
  const userExpectations = [
    [stripe.orderId.slice(0, 12), '已完成'],
    [easyPay.orderId.slice(0, 12), '已完成'],
    [summary.stripeRefundManualRecovery.orderId.slice(0, 12), '已退款'],
  ];
  for (const [prefix, status] of userExpectations) {
    const rowText = await page.locator('body').innerText();
    expect(rowText.includes(`#${prefix}`), `user orders missing ${prefix}`);
    expect(rowText.includes(status), `user orders missing status ${status} for ${prefix}`);
  }
  screenshots.userOrders = await saveShot(page, outputDir, 'user-orders.png');

  await page.goto(`${baseUrl}/admin/orders?token=opay-admin-smoke-token`, { waitUntil: 'networkidle', timeout: 30000 });
  await page.getByRole('heading', { name: '订单管理' }).waitFor({ timeout: 15000 });
  const adminExpectations = [
    [summary.adminOrderActions.seededOrders.cancel.slice(0, 12), '已取消'],
    [summary.adminOrderActions.seededOrders.retry.slice(0, 12), '已完成'],
    [summary.adminOrderActions.seededOrders.refund.slice(0, 12), '已部分退款'],
  ];
  for (const [prefix, status] of adminExpectations) {
    const rowText = await page.locator('body').innerText();
    expect(rowText.includes(prefix), `admin orders missing ${prefix}`);
    expect(rowText.includes(status), `admin orders missing status ${status} for ${prefix}`);
  }
  screenshots.adminOrders = await saveShot(page, outputDir, 'admin-orders.png');

  await browser.close();

  const result = {
    success: true,
    screenshots,
    checked: {
      stripeResultOrderId: stripe.orderId,
      easyPayResultOrderId: easyPay.orderId,
      userOrderRefunded: summary.stripeRefundManualRecovery.orderId,
      adminSeededOrders: summary.adminOrderActions.seededOrders,
    },
  };

  fs.writeFileSync(resultPath, JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result, null, 2));
})().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
