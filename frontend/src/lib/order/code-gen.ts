import crypto from 'crypto';

export function generateRechargeCode(orderId: string): string {
  const prefix = 's2p_';
  const random = crypto.randomBytes(4).toString('hex'); // 8 chars
  const maxIdLength = 32 - prefix.length - random.length; // 16
  const truncatedId = orderId.replace(/-/g, '').slice(0, maxIdLength);
  return `${prefix}${truncatedId}${random}`;
}
