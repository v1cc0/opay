/**
 * Client-safe utility for building order status API URLs.
 * This module must NOT import any server-only modules (config, fs, crypto, etc.).
 */

const ACCESS_TOKEN_KEY = 'access_token';
const TOKEN_KEY = 'token';

export function buildOrderStatusUrl(orderId: string, accessToken?: string | null, token?: string | null): string {
  const query = new URLSearchParams();
  if (accessToken) {
    query.set(ACCESS_TOKEN_KEY, accessToken);
  } else if (token) {
    query.set(TOKEN_KEY, token);
  }
  const suffix = query.toString();
  return suffix ? `/api/orders/${orderId}?${suffix}` : `/api/orders/${orderId}`;
}
