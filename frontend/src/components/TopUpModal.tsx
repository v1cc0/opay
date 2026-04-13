'use client';

import React, { useState } from 'react';
import type { Locale } from '@/lib/locale';
import { pickLocaleText } from '@/lib/locale';

interface TopUpModalProps {
  open: boolean;
  onClose: () => void;
  onConfirm: (amount: number) => void;
  amounts?: number[];
  isDark: boolean;
  locale: Locale;
}

const DEFAULT_AMOUNTS = [50, 100, 500, 1000];

export default function TopUpModal({ open, onClose, onConfirm, amounts, isDark, locale }: TopUpModalProps) {
  const amountOptions = amounts ?? DEFAULT_AMOUNTS;
  const [selected, setSelected] = useState<number | null>(null);

  if (!open) return null;

  const handleConfirm = () => {
    if (selected !== null) {
      onConfirm(selected);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div
        className={[
          'relative mx-4 w-full max-w-md rounded-2xl border p-6 shadow-2xl',
          isDark ? 'border-slate-700 bg-slate-900 text-slate-100' : 'border-slate-200 bg-white text-slate-900',
        ].join(' ')}
      >
        {/* Header */}
        <div className="mb-5 flex items-center justify-between">
          <h2 className="text-lg font-semibold">{pickLocaleText(locale, '选择充值金额', 'Select Amount')}</h2>
          <button
            type="button"
            onClick={onClose}
            className={[
              'flex h-8 w-8 items-center justify-center rounded-full transition-colors',
              isDark
                ? 'text-slate-400 hover:bg-slate-800 hover:text-slate-200'
                : 'text-slate-400 hover:bg-slate-100 hover:text-slate-600',
            ].join(' ')}
          >
            <svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Amount grid */}
        <div className="mb-6 grid grid-cols-2 gap-3">
          {amountOptions.map((amount) => {
            const isSelected = selected === amount;
            return (
              <button
                key={amount}
                type="button"
                onClick={() => setSelected(amount)}
                className={[
                  'flex flex-col items-center rounded-xl border-2 px-4 py-4 transition-all',
                  isSelected
                    ? 'border-emerald-500 ring-2 ring-emerald-500/30'
                    : isDark
                      ? 'border-slate-700 hover:border-slate-600'
                      : 'border-slate-200 hover:border-slate-300',
                  isSelected
                    ? isDark
                      ? 'bg-emerald-950/40'
                      : 'bg-emerald-50'
                    : isDark
                      ? 'bg-slate-800/60'
                      : 'bg-slate-50',
                ].join(' ')}
              >
                <span className={['text-xs', isDark ? 'text-slate-400' : 'text-slate-500'].join(' ')}>
                  {pickLocaleText(locale, `余额充值${amount}$`, `Balance +${amount}$`)}
                </span>
                <span className="mt-1 text-2xl font-bold text-emerald-500">¥{amount}</span>
              </button>
            );
          })}
        </div>

        {/* Confirm button */}
        <button
          type="button"
          disabled={selected === null}
          onClick={handleConfirm}
          className={[
            'w-full rounded-xl py-3 text-sm font-semibold text-white transition-colors',
            selected !== null
              ? 'bg-emerald-500 hover:bg-emerald-600 active:bg-emerald-700'
              : isDark
                ? 'cursor-not-allowed bg-slate-700 text-slate-400'
                : 'cursor-not-allowed bg-slate-200 text-slate-400',
          ].join(' ')}
        >
          {pickLocaleText(locale, '确认充值', 'Confirm')}
        </button>
      </div>
    </div>
  );
}
