import { initPaymentProviders, paymentRegistry } from '@/lib/payment';
import { Prisma } from '@prisma/client';

/**
 * 获取指定支付渠道的手续费率（百分比）。
 * 优先级：FEE_RATE_{TYPE} > FEE_RATE_PROVIDER_{KEY} > 0
 */
export function getMethodFeeRate(paymentType: string): number {
  // 渠道级别：FEE_RATE_ALIPAY / FEE_RATE_WXPAY / FEE_RATE_STRIPE
  const methodRaw = process.env[`FEE_RATE_${paymentType.toUpperCase()}`];
  if (methodRaw !== undefined && methodRaw !== '') {
    const num = Number(methodRaw);
    if (Number.isFinite(num) && num >= 0) return num;
  }

  // 提供商级别：FEE_RATE_PROVIDER_EASYPAY / FEE_RATE_PROVIDER_STRIPE
  initPaymentProviders();
  const providerKey = paymentRegistry.getProviderKey(paymentType);
  if (providerKey) {
    const providerRaw = process.env[`FEE_RATE_PROVIDER_${providerKey.toUpperCase()}`];
    if (providerRaw !== undefined && providerRaw !== '') {
      const num = Number(providerRaw);
      if (Number.isFinite(num) && num >= 0) return num;
    }
  }

  return 0;
}

/** decimal.js ROUND_UP = 0（远离零方向取整） */
const ROUND_UP = 0;

/**
 * 根据到账金额和手续费率计算实付金额（使用 Decimal 精确计算，避免浮点误差）。
 * feeAmount = ceil(rechargeAmount * feeRate / 100, 保留2位小数)
 * payAmount = rechargeAmount + feeAmount
 */
export function calculatePayAmount(rechargeAmount: number, feeRate: number): string {
  if (feeRate <= 0) return rechargeAmount.toFixed(2);
  const amount = new Prisma.Decimal(rechargeAmount);
  const rate = new Prisma.Decimal(feeRate.toString());
  const feeAmount = amount.mul(rate).div(100).toDecimalPlaces(2, ROUND_UP);
  return amount.plus(feeAmount).toFixed(2);
}
