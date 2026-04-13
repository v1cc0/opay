import type { Locale } from '@/lib/locale';

interface Summary {
  total: number;
  pending: number;
  completed: number;
  failed: number;
}

interface OrderSummaryCardsProps {
  isDark: boolean;
  locale: Locale;
  summary: Summary;
}

export default function OrderSummaryCards({ isDark, locale, summary }: OrderSummaryCardsProps) {
  const cardClass = [
    'rounded-xl border p-3',
    isDark ? 'border-slate-700 bg-slate-800/70' : 'border-slate-200 bg-slate-50',
  ].join(' ');
  const labelClass = ['text-xs', isDark ? 'text-slate-400' : 'text-slate-500'].join(' ');
  const labels =
    locale === 'en'
      ? {
          total: 'Total Orders',
          pending: 'Pending',
          completed: 'Completed',
          failed: 'Closed/Failed',
        }
      : {
          total: '总订单',
          pending: '待支付',
          completed: '已完成',
          failed: '异常/关闭',
        };

  return (
    <div className="mb-4 grid grid-cols-2 gap-3 sm:grid-cols-4">
      <div className={cardClass}>
        <div className={labelClass}>{labels.total}</div>
        <div className="mt-1 text-xl font-semibold">{summary.total}</div>
      </div>
      <div className={cardClass}>
        <div className={labelClass}>{labels.pending}</div>
        <div className="mt-1 text-xl font-semibold">{summary.pending}</div>
      </div>
      <div className={cardClass}>
        <div className={labelClass}>{labels.completed}</div>
        <div className="mt-1 text-xl font-semibold">{summary.completed}</div>
      </div>
      <div className={cardClass}>
        <div className={labelClass}>{labels.failed}</div>
        <div className="mt-1 text-xl font-semibold">{summary.failed}</div>
      </div>
    </div>
  );
}
