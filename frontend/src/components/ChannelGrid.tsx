'use client';

import React from 'react';
import type { Locale } from '@/lib/locale';
import ChannelCard from '@/components/ChannelCard';
import type { ChannelInfo } from '@/components/ChannelCard';

interface ChannelGridProps {
  channels: ChannelInfo[];
  onTopUp: () => void;
  isDark: boolean;
  locale: Locale;
  userBalance?: number;
}

export type { ChannelInfo };

export default function ChannelGrid({ channels, onTopUp, isDark, locale, userBalance }: ChannelGridProps) {
  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
      {channels.map((channel) => (
        <ChannelCard
          key={channel.id}
          channel={channel}
          onTopUp={onTopUp}
          isDark={isDark}
          locale={locale}
          userBalance={userBalance}
        />
      ))}
    </div>
  );
}
