'use client';

import { useState } from 'react';
import type { Locale } from '@/lib/locale';
import { PAYMENT_TYPE_META, getPaymentIconType, getPaymentMeta, getPaymentDisplayInfo } from '@/lib/pay-utils';

export interface MethodLimitInfo {
  available: boolean;
  remaining: number | null;
  /** 单笔最小限额，0 = 使用全局 minAmount */
  singleMin?: number;
  /** 单笔最大限额，0 = 使用全局 maxAmount */
  singleMax?: number;
  /** 手续费率百分比，0 = 无手续费 */
  feeRate?: number;
}

interface PaymentFormProps {
  userId: number;
  userName?: string;
  userBalance?: number;
  enabledPaymentTypes: string[];
  methodLimits?: Record<string, MethodLimitInfo>;
  minAmount: number;
  maxAmount: number;
  onSubmit: (amount: number, paymentType: string) => Promise<void>;
  loading?: boolean;
  dark?: boolean;
  pendingBlocked?: boolean;
  pendingCount?: number;
  locale?: Locale;
  /** 固定金额模式：隐藏金额选择，只显示支付方式和提交按钮 */
  fixedAmount?: number;
}

const QUICK_AMOUNTS = [10, 20, 50, 100, 200, 500, 1000, 2000];
const AMOUNT_TEXT_PATTERN = /^\d*(\.\d{0,2})?$/;

function hasValidCentPrecision(num: number): boolean {
  return Math.abs(Math.round(num * 100) - num * 100) < 1e-8;
}

export default function PaymentForm({
  userId,
  userName,
  userBalance,
  enabledPaymentTypes,
  methodLimits,
  minAmount,
  maxAmount,
  onSubmit,
  loading,
  dark = false,
  pendingBlocked = false,
  pendingCount = 0,
  locale = 'zh',
  fixedAmount,
}: PaymentFormProps) {
  const [amount, setAmount] = useState<number | ''>(fixedAmount ?? '');
  const [paymentType, setPaymentType] = useState(enabledPaymentTypes[0] || 'alipay');
  const [customAmount, setCustomAmount] = useState(fixedAmount ? String(fixedAmount) : '');

  const effectivePaymentType = enabledPaymentTypes.includes(paymentType)
    ? paymentType
    : enabledPaymentTypes[0] || 'stripe';

  const handleQuickAmount = (val: number) => {
    setAmount(val);
    setCustomAmount(String(val));
  };

  const handleCustomAmountChange = (val: string) => {
    if (!AMOUNT_TEXT_PATTERN.test(val)) {
      return;
    }

    setCustomAmount(val);

    if (val === '') {
      setAmount('');
      return;
    }

    const num = parseFloat(val);
    if (!isNaN(num) && num > 0 && hasValidCentPrecision(num)) {
      setAmount(num);
    } else {
      setAmount('');
    }
  };

  const selectedAmount = amount || 0;
  const isMethodAvailable = !methodLimits || methodLimits[effectivePaymentType]?.available !== false;
  const methodSingleMax = methodLimits?.[effectivePaymentType]?.singleMax;
  const methodSingleMin = methodLimits?.[effectivePaymentType]?.singleMin;
  const effectiveMax = methodSingleMax !== undefined && methodSingleMax > 0 ? methodSingleMax : maxAmount;
  const effectiveMin =
    methodSingleMin !== undefined && methodSingleMin > 0 ? Math.max(methodSingleMin, minAmount) : minAmount;
  const feeRate = methodLimits?.[effectivePaymentType]?.feeRate ?? 0;
  const feeAmount = feeRate > 0 && selectedAmount > 0 ? Math.ceil(((selectedAmount * feeRate) / 100) * 100) / 100 : 0;
  const payAmount =
    feeRate > 0 && selectedAmount > 0 ? Math.round((selectedAmount + feeAmount) * 100) / 100 : selectedAmount;
  const isValid =
    selectedAmount >= effectiveMin &&
    selectedAmount <= effectiveMax &&
    hasValidCentPrecision(selectedAmount) &&
    isMethodAvailable;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!isValid || loading) return;
    await onSubmit(selectedAmount, effectivePaymentType);
  };

  const renderPaymentIcon = (type: string) => {
    const iconType = getPaymentIconType(type);
    if (iconType === 'alipay') {
      return (
        <span className="flex h-8 w-8 items-center justify-center rounded-md bg-[#00AEEF] text-xl font-bold leading-none text-white">
          {locale === 'en' ? 'A' : '支'}
        </span>
      );
    }
    if (iconType === 'wxpay') {
      return (
        <span className="flex h-8 w-8 items-center justify-center rounded-full bg-[#07C160] text-white">
          <svg viewBox="0 0 24 24" className="h-5 w-5" fill="currentColor">
            <path d="M10 3C6.13 3 3 5.58 3 8.75c0 1.7.84 3.23 2.17 4.29l-.5 2.21 2.4-1.32c.61.17 1.25.27 1.93.27.22 0 .43-.01.64-.03C9.41 13.72 9 12.88 9 12c0-3.31 3.13-6 7-6 .26 0 .51.01.76.03C15.96 3.98 13.19 3 10 3z" />
            <path d="M16 8c-3.31 0-6 2.24-6 5s2.69 5 6 5c.67 0 1.31-.1 1.9-.28l2.1 1.15-.55-2.44C20.77 15.52 22 13.86 22 12c0-2.21-2.69-4-6-4z" />
          </svg>
        </span>
      );
    }
    if (iconType === 'stripe') {
      return (
        <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-[#635bff] text-white">
          <svg
            viewBox="0 0 24 24"
            className="h-5 w-5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <rect x="2" y="5" width="20" height="14" rx="2" />
            <path d="M2 10h20" />
          </svg>
        </span>
      );
    }
    return null;
  };

  return (
    <form onSubmit={handleSubmit} className="space-y-6">
      <div
        className={[
          'rounded-xl border p-4',
          dark ? 'border-slate-700 bg-slate-800/80' : 'border-slate-200 bg-slate-50',
        ].join(' ')}
      >
        <div className={['text-xs uppercase tracking-wide', dark ? 'text-slate-400' : 'text-slate-500'].join(' ')}>
          {locale === 'en' ? 'Recharge Account' : '充值账户'}
        </div>
        <div className={['mt-1 text-base font-medium', dark ? 'text-slate-100' : 'text-slate-900'].join(' ')}>
          {userName || (locale === 'en' ? `User #${userId}` : `用户 #${userId}`)}
        </div>
        {userBalance !== undefined && (
          <div className={['mt-1 text-sm', dark ? 'text-slate-400' : 'text-slate-500'].join(' ')}>
            {locale === 'en' ? 'Current Balance:' : '当前余额:'}{' '}
            <span className="font-medium text-green-600">{userBalance.toFixed(2)}</span>
          </div>
        )}
      </div>

      {fixedAmount ? (
        <div
          className={[
            'rounded-xl border p-4 text-center',
            dark ? 'border-slate-700 bg-slate-800/60' : 'border-slate-200 bg-slate-50',
          ].join(' ')}
        >
          <div className={['text-xs uppercase tracking-wide', dark ? 'text-slate-400' : 'text-slate-500'].join(' ')}>
            {locale === 'en' ? 'Recharge Amount' : '充值金额'}
          </div>
          <div className={['mt-1 text-3xl font-bold', dark ? 'text-emerald-400' : 'text-emerald-600'].join(' ')}>
            ¥{fixedAmount.toFixed(2)}
          </div>
        </div>
      ) : (
        <>
          <div>
            <label className={['mb-2 block text-sm font-medium', dark ? 'text-slate-200' : 'text-slate-700'].join(' ')}>
              {locale === 'en' ? 'Recharge Amount' : '充值金额'}
            </label>
            <div className="grid grid-cols-3 gap-2">
              {QUICK_AMOUNTS.filter((val) => val >= effectiveMin && val <= effectiveMax).map((val) => (
                <button
                  key={val}
                  type="button"
                  onClick={() => handleQuickAmount(val)}
                  className={`rounded-lg border-2 px-4 py-3 text-center font-medium transition-colors ${
                    amount === val
                      ? dark
                        ? 'border-blue-500 bg-blue-900/40 text-blue-300'
                        : 'border-blue-500 bg-blue-50 text-blue-700'
                      : dark
                        ? 'border-slate-700 bg-slate-900 text-slate-200 hover:border-slate-500'
                        : 'border-gray-200 bg-white text-gray-700 hover:border-gray-300'
                  }`}
                >
                  ¥{val}
                </button>
              ))}
            </div>
          </div>

          <div>
            <label className={['mb-2 block text-sm font-medium', dark ? 'text-slate-200' : 'text-slate-700'].join(' ')}>
              {locale === 'en' ? 'Custom Amount' : '自定义金额'}
            </label>
            <div className="relative">
              <span
                className={['absolute left-3 top-1/2 -translate-y-1/2', dark ? 'text-slate-500' : 'text-gray-400'].join(
                  ' ',
                )}
              >
                ¥
              </span>
              <input
                type="text"
                inputMode="decimal"
                step="0.01"
                min={effectiveMin}
                max={effectiveMax}
                value={customAmount}
                onChange={(e) => handleCustomAmountChange(e.target.value)}
                placeholder={`${effectiveMin} - ${effectiveMax}`}
                className={[
                  'w-full rounded-lg border py-3 pl-8 pr-4 focus:border-blue-500 focus:outline-none focus:ring-1 focus:ring-blue-500',
                  dark ? 'border-slate-700 bg-slate-900 text-slate-100' : 'border-gray-300 bg-white text-gray-900',
                ].join(' ')}
              />
            </div>
          </div>
        </>
      )}

      {!fixedAmount &&
        customAmount !== '' &&
        !isValid &&
        (() => {
          const num = parseFloat(customAmount);
          let msg =
            locale === 'en'
              ? 'Amount must be within range and support up to 2 decimal places'
              : '金额需在范围内，且最多支持 2 位小数（精确到分）';
          if (!isNaN(num)) {
            if (num < minAmount)
              msg = locale === 'en' ? `Minimum per transaction: ¥${minAmount}` : `单笔最低充值 ¥${minAmount}`;
            else if (num > effectiveMax)
              msg = locale === 'en' ? `Maximum per transaction: ¥${effectiveMax}` : `单笔最高充值 ¥${effectiveMax}`;
          }
          return <div className={['text-xs', dark ? 'text-amber-300' : 'text-amber-700'].join(' ')}>{msg}</div>;
        })()}

      {enabledPaymentTypes.length > 1 && (
        <div>
          <label className={['mb-2 block text-sm font-medium', dark ? 'text-slate-200' : 'text-gray-700'].join(' ')}>
            {locale === 'en' ? 'Payment Method' : '支付方式'}
          </label>
          <div className="grid grid-cols-2 gap-3 sm:flex">
            {enabledPaymentTypes.map((type) => {
              const meta = PAYMENT_TYPE_META[type];
              const displayInfo = getPaymentDisplayInfo(type, locale);
              const isSelected = effectivePaymentType === type;
              const limitInfo = methodLimits?.[type];
              const isUnavailable = limitInfo !== undefined && !limitInfo.available;

              return (
                <button
                  key={type}
                  type="button"
                  disabled={isUnavailable}
                  onClick={() => !isUnavailable && setPaymentType(type)}
                  title={
                    isUnavailable
                      ? locale === 'en'
                        ? 'Daily limit reached, please use another payment method'
                        : '今日充值额度已满，请使用其他支付方式'
                      : undefined
                  }
                  className={[
                    'relative flex h-[58px] flex-col items-center justify-center rounded-lg border px-3 transition-all sm:flex-1',
                    isUnavailable
                      ? dark
                        ? 'cursor-not-allowed border-slate-700 bg-slate-800/50 opacity-50'
                        : 'cursor-not-allowed border-gray-200 bg-gray-50 opacity-50'
                      : isSelected
                        ? `${meta?.selectedBorder || 'border-blue-500'} ${dark ? meta?.selectedBgDark || 'bg-blue-950' : meta?.selectedBg || 'bg-blue-50'} ${dark ? 'text-slate-100' : 'text-slate-900'} shadow-sm`
                        : dark
                          ? 'border-slate-700 bg-slate-900 text-slate-200 hover:border-slate-500'
                          : 'border-gray-300 bg-white text-slate-700 hover:border-gray-400',
                  ].join(' ')}
                >
                  <span className="flex items-center gap-2">
                    {renderPaymentIcon(type)}
                    <span className="flex flex-col items-start leading-none">
                      <span className="text-xl font-semibold tracking-tight">{displayInfo.channel || type}</span>
                      {isUnavailable ? (
                        <span className="text-[10px] tracking-wide text-red-400">
                          {locale === 'en' ? 'Daily limit reached' : '今日额度已满'}
                        </span>
                      ) : displayInfo.sublabel ? (
                        <span
                          className={`text-[10px] tracking-wide ${dark ? (isSelected ? 'text-slate-300' : 'text-slate-400') : 'text-slate-600'}`}
                        >
                          {displayInfo.sublabel}
                        </span>
                      ) : null}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>

          {(() => {
            const limitInfo = methodLimits?.[effectivePaymentType];
            if (!limitInfo || limitInfo.available) return null;
            return (
              <p className={['mt-2 text-xs', dark ? 'text-amber-300' : 'text-amber-600'].join(' ')}>
                {locale === 'en'
                  ? "The selected payment method has reached today's limit. Please switch to another method."
                  : '所选支付方式今日额度已满，请切换到其他支付方式'}
              </p>
            );
          })()}
        </div>
      )}

      {feeRate > 0 && selectedAmount > 0 && (
        <div
          className={[
            'rounded-xl border px-4 py-3 text-sm',
            dark ? 'border-slate-700 bg-slate-800/60 text-slate-300' : 'border-slate-200 bg-slate-50 text-slate-600',
          ].join(' ')}
        >
          <div className="flex items-center justify-between">
            <span>{locale === 'en' ? 'Recharge Amount' : '充值金额'}</span>
            <span>¥{selectedAmount.toFixed(2)}</span>
          </div>
          <div className="mt-1 flex items-center justify-between">
            <span>{locale === 'en' ? `Fee (${feeRate}%)` : `手续费（${feeRate}%）`}</span>
            <span>¥{feeAmount.toFixed(2)}</span>
          </div>
          <div
            className={[
              'mt-1.5 flex items-center justify-between border-t pt-1.5 font-medium',
              dark ? 'border-slate-700 text-slate-100' : 'border-slate-200 text-slate-900',
            ].join(' ')}
          >
            <span>{locale === 'en' ? 'Amount to Pay' : '实付金额'}</span>
            <span>¥{payAmount.toFixed(2)}</span>
          </div>
        </div>
      )}

      {pendingBlocked && (
        <div
          className={[
            'rounded-lg border p-3 text-sm',
            dark ? 'border-amber-700 bg-amber-900/30 text-amber-300' : 'border-amber-200 bg-amber-50 text-amber-700',
          ].join(' ')}
        >
          {locale === 'en'
            ? `You have ${pendingCount} pending orders. Please complete or cancel them before recharging.`
            : `您有 ${pendingCount} 个待支付订单，请先完成或取消后再充值`}
        </div>
      )}

      <button
        type="submit"
        disabled={!isValid || loading || pendingBlocked}
        className={`w-full rounded-lg py-3 text-center font-medium transition-colors ${
          isValid && !loading && !pendingBlocked
            ? `text-white ${getPaymentMeta(effectivePaymentType).buttonClass}`
            : dark
              ? 'cursor-not-allowed bg-slate-700 text-slate-400'
              : 'cursor-not-allowed bg-gray-300 text-gray-500'
        }`}
      >
        {loading
          ? locale === 'en'
            ? 'Processing...'
            : '处理中...'
          : pendingBlocked
            ? locale === 'en'
              ? 'Too many pending orders'
              : '待支付订单过多'
            : locale === 'en'
              ? `Recharge Now ¥${(feeRate > 0 && selectedAmount > 0 ? payAmount : selectedAmount || 0).toFixed(2)}`
              : `立即充值 ¥${(feeRate > 0 && selectedAmount > 0 ? payAmount : selectedAmount || 0).toFixed(2)}`}
      </button>
    </form>
  );
}
