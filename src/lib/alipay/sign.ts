import crypto from 'crypto';

/** 将裸 base64 按 64 字符/行折行，符合 PEM 标准（OpenSSL 3.x 严格模式要求） */
function wrapBase64(b64: string): string {
  return b64.replace(/(.{64})/g, '$1\n').trim();
}

function normalizePemLikeValue(key: string): string {
  return key
    .trim()
    .replace(/\r\n/g, '\n')
    .replace(/\\r\\n/g, '\n')
    .replace(/\\n/g, '\n');
}

function shouldLogVerifyDebug(): boolean {
  return process.env.NODE_ENV !== 'production' || process.env.DEBUG_ALIPAY_SIGN === '1';
}

/** 自动补全 PEM 格式（PKCS8） */
function formatPrivateKey(key: string): string {
  const normalized = normalizePemLikeValue(key);
  if (normalized.includes('-----BEGIN')) return normalized;
  return `-----BEGIN PRIVATE KEY-----\n${wrapBase64(normalized)}\n-----END PRIVATE KEY-----`;
}

function formatPublicKey(key: string): string {
  const normalized = normalizePemLikeValue(key);
  if (normalized.includes('-----BEGIN')) return normalized;
  return `-----BEGIN PUBLIC KEY-----\n${wrapBase64(normalized)}\n-----END PUBLIC KEY-----`;
}

/** 生成 RSA2 签名（请求签名：仅排除 sign） */
export function generateSign(params: Record<string, string>, privateKey: string): string {
  const filtered = Object.entries(params)
    .filter(([key, value]) => key !== 'sign' && value !== '' && value !== undefined && value !== null)
    .sort(([a], [b]) => a.localeCompare(b));

  const signStr = filtered.map(([key, value]) => `${key}=${value}`).join('&');

  const signer = crypto.createSign('RSA-SHA256');
  signer.update(signStr);
  return signer.sign(formatPrivateKey(privateKey), 'base64');
}

/**
 * 验证支付宝服务端 API 响应签名。
 * 从原始 JSON 文本中提取 responseKey 对应的子串作为验签内容。
 */
export function verifyResponseSign(
  rawText: string,
  responseKey: string,
  alipayPublicKey: string,
  sign: string,
): boolean {
  // 从原始文本中精确提取 responseKey 对应的 JSON 子串
  // 格式: {"responseKey":{ ... },"sign":"..."}
  const keyPattern = `"${responseKey}"`;
  const keyIdx = rawText.indexOf(keyPattern);
  if (keyIdx < 0) return false;

  const colonIdx = rawText.indexOf(':', keyIdx + keyPattern.length);
  if (colonIdx < 0) return false;

  // 找到 value 的起始位置（跳过冒号后的空白）
  let start = colonIdx + 1;
  while (start < rawText.length && rawText[start] === ' ') start++;

  // 使用括号匹配找到完整的 JSON 值
  let depth = 0;
  let end = start;
  let inString = false;
  let escaped = false;
  for (let i = start; i < rawText.length; i++) {
    const ch = rawText[i];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === '\\' && inString) {
      escaped = true;
      continue;
    }
    if (ch === '"') {
      inString = !inString;
      continue;
    }
    if (inString) continue;
    if (ch === '{') depth++;
    if (ch === '}') {
      depth--;
      if (depth === 0) {
        end = i + 1;
        break;
      }
    }
  }

  const signContent = rawText.substring(start, end);
  const pem = formatPublicKey(alipayPublicKey);
  try {
    const verifier = crypto.createVerify('RSA-SHA256');
    verifier.update(signContent);
    return verifier.verify(pem, sign, 'base64');
  } catch (err) {
    if (shouldLogVerifyDebug()) {
      console.error('[Alipay verifyResponseSign] crypto error:', err);
    }
    return false;
  }
}

/** 用支付宝公钥验证签名（回调验签：排除 sign 和 sign_type） */
export function verifySign(params: Record<string, string>, alipayPublicKey: string, sign: string): boolean {
  const filtered = Object.entries(params)
    .filter(
      ([key, value]) => key !== 'sign' && key !== 'sign_type' && value !== '' && value !== undefined && value !== null,
    )
    .sort(([a], [b]) => a.localeCompare(b));

  const signStr = filtered.map(([key, value]) => `${key}=${value}`).join('&');

  const pem = formatPublicKey(alipayPublicKey);
  try {
    const verifier = crypto.createVerify('RSA-SHA256');
    verifier.update(signStr);
    const result = verifier.verify(pem, sign, 'base64');
    if (!result) {
      if (shouldLogVerifyDebug()) {
        console.error('[Alipay verifySign] FAILED. signStr:', signStr.substring(0, 200) + '...');
        console.error('[Alipay verifySign] sign(first 40):', sign.substring(0, 40));
        console.error('[Alipay verifySign] pubKey(first 80):', pem.substring(0, 80));
      } else {
        console.error('[Alipay verifySign] verification failed');
      }
    }
    return result;
  } catch (err) {
    if (shouldLogVerifyDebug()) {
      console.error('[Alipay verifySign] crypto error:', err);
    } else {
      const message = err instanceof Error ? err.message : String(err);
      console.error('[Alipay verifySign] crypto error:', message);
    }
    return false;
  }
}
