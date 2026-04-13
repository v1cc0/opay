import crypto from 'crypto';
import { getEnv } from '@/lib/config';

export const ORDER_STATUS_ACCESS_QUERY_KEY = 'access_token';
const ORDER_STATUS_ACCESS_PURPOSE = 'order-status-access:v2';
/** access_token 有效期（24 小时） */
const TOKEN_TTL_MS = 24 * 60 * 60 * 1000;

/** 使用独立派生密钥，不直接使用 ADMIN_TOKEN */
function deriveKey(): string {
  return crypto.createHmac('sha256', getEnv().ADMIN_TOKEN).update('order-status-access-key').digest('hex');
}

function buildSignature(orderId: string, userId: number, expiresAt: number): string {
  return crypto
    .createHmac('sha256', deriveKey())
    .update(`${ORDER_STATUS_ACCESS_PURPOSE}:${orderId}:${userId}:${expiresAt}`)
    .digest('base64url');
}

/** 生成格式: {expiresAt}.{userId}.{signature} */
export function createOrderStatusAccessToken(orderId: string, userId?: number): string {
  const expiresAt = Date.now() + TOKEN_TTL_MS;
  const uid = userId ?? 0;
  const sig = buildSignature(orderId, uid, expiresAt);
  return `${expiresAt}.${uid}.${sig}`;
}

export function verifyOrderStatusAccessToken(orderId: string, token: string | null | undefined): boolean {
  if (!token) return false;

  const parts = token.split('.');
  if (parts.length !== 3) return false;

  const [expiresAtStr, userIdStr, sig] = parts;
  const expiresAt = Number(expiresAtStr);
  const userId = Number(userIdStr);

  if (!Number.isFinite(expiresAt) || !Number.isFinite(userId)) return false;

  // 检查过期
  if (Date.now() > expiresAt) return false;

  const expected = buildSignature(orderId, userId, expiresAt);
  const expectedBuffer = Buffer.from(expected, 'utf8');
  const receivedBuffer = Buffer.from(sig, 'utf8');

  if (expectedBuffer.length !== receivedBuffer.length) {
    return false;
  }

  return crypto.timingSafeEqual(expectedBuffer, receivedBuffer);
}

export function buildOrderResultUrl(appUrl: string, orderId: string, userId?: number): string {
  const url = new URL('/pay/result', appUrl);
  url.searchParams.set('order_id', orderId);
  url.searchParams.set(ORDER_STATUS_ACCESS_QUERY_KEY, createOrderStatusAccessToken(orderId, userId));
  return url.toString();
}
