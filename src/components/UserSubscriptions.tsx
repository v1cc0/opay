'use client';

import React from 'react';
import type { Locale } from '@/lib/locale';
import { pickLocaleText } from '@/lib/locale';
import { PlatformBadge } from '@/lib/platform-style';

export interface UserSub {
  id: number;
  group_id: number;
  starts_at: string;
  expires_at: string;
  status: string;
  daily_usage_usd: number;
  weekly_usage_usd: number;
  monthly_usage_usd: number;
  group_name: string | null;
  platform: string | null;
}

interface UserSubscriptionsProps {
  subscriptions: UserSub[];
  onRenew: (groupId: number) => void;
  isDark: boolean;
  locale: Locale;
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleDateString('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit' });
}

function daysUntil(iso: string): number {
  const now = new Date();
  const target = new Date(iso);
  return Math.ceil((target.getTime() - now.getTime()) / (1000 * 60 * 60 * 24));
}

function getStatusBadge(status: string, isDark: boolean, locale: Locale): { text: string; className: string } {
  const statusMap: Record<string, { zh: string; en: string; cls: string; clsDark: string }> = {
    active: {
      zh: '生效中',
      en: 'Active',
      cls: 'bg-emerald-100 text-emerald-700',
      clsDark: 'bg-emerald-900/40 text-emerald-300',
    },
    expired: {
      zh: '已过期',
      en: 'Expired',
      cls: 'bg-slate-100 text-slate-600',
      clsDark: 'bg-slate-700 text-slate-400',
    },
    cancelled: { zh: '已取消', en: 'Cancelled', cls: 'bg-red-100 text-red-700', clsDark: 'bg-red-900/40 text-red-300' },
  };
  const entry = statusMap[status] || {
    zh: status,
    en: status,
    cls: 'bg-slate-100 text-slate-600',
    clsDark: 'bg-slate-700 text-slate-400',
  };
  return {
    text: pickLocaleText(locale, entry.zh, entry.en),
    className: isDark ? entry.clsDark : entry.cls,
  };
}

export default function UserSubscriptions({ subscriptions, onRenew, isDark, locale }: UserSubscriptionsProps) {
  if (subscriptions.length === 0) {
    return (
      <div
        className={[
          'flex flex-col items-center justify-center rounded-2xl border py-16',
          isDark ? 'border-slate-700 bg-slate-800/50 text-slate-400' : 'border-slate-200 bg-slate-50 text-slate-500',
        ].join(' ')}
      >
        <svg
          className="mb-3 h-12 w-12 opacity-40"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={1.5}
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
          />
        </svg>
        <p className="text-sm">{pickLocaleText(locale, '暂无订阅', 'No Subscriptions')}</p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {subscriptions.map((sub) => {
        const remaining = daysUntil(sub.expires_at);
        const isExpiringSoon = remaining > 0 && remaining <= 7;
        const badge = getStatusBadge(sub.status, isDark, locale);

        return (
          <div
            key={sub.id}
            className={[
              'rounded-2xl border p-4',
              isDark ? 'border-slate-700 bg-slate-800/70' : 'border-slate-200 bg-white',
            ].join(' ')}
          >
            {/* Header */}
            <div className="mb-3 flex items-center justify-between">
              <div className="flex items-center gap-2">
                {sub.platform && <PlatformBadge platform={sub.platform} />}
                <span className={['text-base font-semibold', isDark ? 'text-slate-100' : 'text-slate-900'].join(' ')}>
                  {sub.group_name || pickLocaleText(locale, `#${sub.group_id}`, `#${sub.group_id}`)}
                </span>
                <span className={['rounded-full px-2 py-0.5 text-xs font-medium', badge.className].join(' ')}>
                  {badge.text}
                </span>
              </div>
              {sub.status === 'active' && (
                <button
                  type="button"
                  onClick={() => onRenew(sub.group_id)}
                  className={[
                    'rounded-lg px-3 py-1.5 text-xs font-semibold text-white transition-colors',
                    isDark
                      ? 'bg-emerald-500/80 hover:bg-emerald-500 active:bg-emerald-600'
                      : 'bg-emerald-500 hover:bg-emerald-600 active:bg-emerald-700',
                  ].join(' ')}
                >
                  {pickLocaleText(locale, '续费', 'Renew')}
                </button>
              )}
            </div>

            {/* Dates */}
            <div
              className={['mb-3 grid grid-cols-2 gap-3 text-sm', isDark ? 'text-slate-400' : 'text-slate-500'].join(
                ' ',
              )}
            >
              <div>
                <span
                  className={['text-xs uppercase tracking-wide', isDark ? 'text-slate-500' : 'text-slate-400'].join(
                    ' ',
                  )}
                >
                  {pickLocaleText(locale, '开始', 'Start')}
                </span>
                <p className={['font-medium', isDark ? 'text-slate-300' : 'text-slate-700'].join(' ')}>
                  {formatDate(sub.starts_at)}
                </p>
              </div>
              <div>
                <span
                  className={['text-xs uppercase tracking-wide', isDark ? 'text-slate-500' : 'text-slate-400'].join(
                    ' ',
                  )}
                >
                  {pickLocaleText(locale, '到期', 'Expires')}
                </span>
                <p className={['font-medium', isDark ? 'text-slate-300' : 'text-slate-700'].join(' ')}>
                  {formatDate(sub.expires_at)}
                </p>
              </div>
            </div>

            {/* Expiry warning */}
            {isExpiringSoon && (
              <div
                className={[
                  'mb-3 rounded-lg px-3 py-2 text-xs font-medium',
                  isDark ? 'bg-amber-900/30 text-amber-300' : 'bg-amber-50 text-amber-700',
                ].join(' ')}
              >
                {pickLocaleText(locale, `即将到期，剩余 ${remaining} 天`, `Expiring soon, ${remaining} days remaining`)}
              </div>
            )}

            {/* Usage stats */}
            <div
              className={[
                'grid grid-cols-3 gap-2 rounded-lg p-3 text-center text-xs',
                isDark ? 'bg-slate-900/60' : 'bg-slate-50',
              ].join(' ')}
            >
              <div>
                <span className={isDark ? 'text-slate-500' : 'text-slate-400'}>
                  {pickLocaleText(locale, '日用量', 'Daily')}
                </span>
                <p className={['mt-0.5 font-semibold', isDark ? 'text-slate-200' : 'text-slate-700'].join(' ')}>
                  ${sub.daily_usage_usd.toFixed(2)}
                </p>
              </div>
              <div>
                <span className={isDark ? 'text-slate-500' : 'text-slate-400'}>
                  {pickLocaleText(locale, '周用量', 'Weekly')}
                </span>
                <p className={['mt-0.5 font-semibold', isDark ? 'text-slate-200' : 'text-slate-700'].join(' ')}>
                  ${sub.weekly_usage_usd.toFixed(2)}
                </p>
              </div>
              <div>
                <span className={isDark ? 'text-slate-500' : 'text-slate-400'}>
                  {pickLocaleText(locale, '月用量', 'Monthly')}
                </span>
                <p className={['mt-0.5 font-semibold', isDark ? 'text-slate-200' : 'text-slate-700'].join(' ')}>
                  ${sub.monthly_usage_usd.toFixed(2)}
                </p>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
