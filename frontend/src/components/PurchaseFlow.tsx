'use client';

import React from 'react';
import type { Locale } from '@/lib/locale';
import { pickLocaleText } from '@/lib/locale';

interface PurchaseFlowProps {
  isDark: boolean;
  locale: Locale;
}

interface Step {
  icon: React.ReactNode;
  zh: string;
  en: string;
}

const STEPS: Step[] = [
  {
    icon: (
      <svg className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.8}>
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M9 5H7a2 2 0 00-2 2v12a2 2 0 002 2h10a2 2 0 002-2V7a2 2 0 00-2-2h-2M9 5a2 2 0 002 2h2a2 2 0 002-2M9 5a2 2 0 012-2h2a2 2 0 012 2m-3 7h3m-3 4h3m-6-4h.01M9 16h.01"
        />
      </svg>
    ),
    zh: '选择套餐',
    en: 'Select Plan',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.8}>
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M3 10h18M7 15h1m4 0h1m-7 4h12a3 3 0 003-3V8a3 3 0 00-3-3H6a3 3 0 00-3 3v8a3 3 0 003 3z"
        />
      </svg>
    ),
    zh: '完成支付',
    en: 'Complete Payment',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.8}>
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M15 7a2 2 0 012 2m4 0a6 6 0 01-7.743 5.743L11 17H9v2H7v2H4a1 1 0 01-1-1v-2.586a1 1 0 01.293-.707l5.964-5.964A6 6 0 1121 9z"
        />
      </svg>
    ),
    zh: '获取激活码',
    en: 'Get Activation',
  },
  {
    icon: (
      <svg className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.8}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
      </svg>
    ),
    zh: '激活使用',
    en: 'Start Using',
  },
];

export default function PurchaseFlow({ isDark, locale }: PurchaseFlowProps) {
  return (
    <div
      className={[
        'rounded-2xl border p-6',
        isDark ? 'border-slate-700 bg-slate-800/50' : 'border-slate-200 bg-slate-50',
      ].join(' ')}
    >
      <h3 className={['mb-5 text-center text-sm font-medium', isDark ? 'text-slate-400' : 'text-slate-500'].join(' ')}>
        {pickLocaleText(locale, '购买流程', 'How It Works')}
      </h3>

      {/* Desktop: horizontal */}
      <div className="hidden items-center justify-center sm:flex">
        {STEPS.map((step, idx) => (
          <React.Fragment key={idx}>
            {/* Step */}
            <div className="flex flex-col items-center gap-2">
              <div
                className={[
                  'flex h-12 w-12 items-center justify-center rounded-full',
                  isDark ? 'bg-emerald-900/40 text-emerald-400' : 'bg-emerald-100 text-emerald-600',
                ].join(' ')}
              >
                {step.icon}
              </div>
              <span className={['text-xs font-medium', isDark ? 'text-slate-300' : 'text-slate-600'].join(' ')}>
                {pickLocaleText(locale, step.zh, step.en)}
              </span>
            </div>

            {/* Connector */}
            {idx < STEPS.length - 1 && (
              <div className={['mx-4 h-px w-12 flex-shrink-0', isDark ? 'bg-slate-700' : 'bg-slate-300'].join(' ')} />
            )}
          </React.Fragment>
        ))}
      </div>

      {/* Mobile: vertical */}
      <div className="flex flex-col items-start gap-0 sm:hidden">
        {STEPS.map((step, idx) => (
          <React.Fragment key={idx}>
            {/* Step */}
            <div className="flex items-center gap-3">
              <div
                className={[
                  'flex h-10 w-10 shrink-0 items-center justify-center rounded-full',
                  isDark ? 'bg-emerald-900/40 text-emerald-400' : 'bg-emerald-100 text-emerald-600',
                ].join(' ')}
              >
                {step.icon}
              </div>
              <span className={['text-sm font-medium', isDark ? 'text-slate-300' : 'text-slate-600'].join(' ')}>
                {pickLocaleText(locale, step.zh, step.en)}
              </span>
            </div>

            {/* Connector */}
            {idx < STEPS.length - 1 && (
              <div className={['ml-5 h-6 w-px', isDark ? 'bg-slate-700' : 'bg-slate-300'].join(' ')} />
            )}
          </React.Fragment>
        ))}
      </div>
    </div>
  );
}
