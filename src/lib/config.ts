import { z } from 'zod';
import fs from 'fs';

const optionalTrimmedString = z.preprocess((value) => {
  if (typeof value !== 'string') return value;
  const trimmed = value.trim();
  return trimmed === '' ? undefined : trimmed;
}, z.string().optional());

const envSchema = z.object({
  DATABASE_URL: z.string().min(1),

  SUB2API_BASE_URL: z.string().url(),
  SUB2API_ADMIN_API_KEY: z.string().min(1),

  // ── 支付服务商（显式声明启用哪些服务商，逗号分隔：easypay, alipay, wxpay, stripe） ──
  PAYMENT_PROVIDERS: z
    .string()
    .default('')
    .transform((v) =>
      v
        .split(',')
        .map((s) => s.trim().toLowerCase())
        .filter(Boolean),
    ),

  // ── Easy-Pay（PAYMENT_PROVIDERS 含 easypay 时必填） ──
  EASY_PAY_PID: optionalTrimmedString,
  EASY_PAY_PKEY: optionalTrimmedString,
  EASY_PAY_API_BASE: optionalTrimmedString,
  EASY_PAY_NOTIFY_URL: optionalTrimmedString,
  EASY_PAY_RETURN_URL: optionalTrimmedString,
  EASY_PAY_CID: optionalTrimmedString,
  EASY_PAY_CID_ALIPAY: optionalTrimmedString,
  EASY_PAY_CID_WXPAY: optionalTrimmedString,

  // ── 支付宝直连（PAYMENT_PROVIDERS 含 alipay 时必填） ──
  // 支持直接传密钥内容，也支持传文件路径（自动读取）
  ALIPAY_APP_ID: optionalTrimmedString,
  ALIPAY_PRIVATE_KEY: optionalTrimmedString,
  ALIPAY_PUBLIC_KEY: optionalTrimmedString,
  ALIPAY_NOTIFY_URL: optionalTrimmedString,
  ALIPAY_RETURN_URL: optionalTrimmedString,

  // ── 微信支付直连（PAYMENT_PROVIDERS 含 wxpay 时必填） ──
  WXPAY_APP_ID: optionalTrimmedString,
  WXPAY_MCH_ID: optionalTrimmedString,
  WXPAY_PRIVATE_KEY: optionalTrimmedString,
  WXPAY_CERT_SERIAL: optionalTrimmedString,
  WXPAY_API_V3_KEY: optionalTrimmedString,
  WXPAY_NOTIFY_URL: optionalTrimmedString,
  WXPAY_PUBLIC_KEY: optionalTrimmedString,
  WXPAY_PUBLIC_KEY_ID: optionalTrimmedString,

  // ── Stripe（PAYMENT_PROVIDERS 含 stripe 时必填） ──
  STRIPE_SECRET_KEY: optionalTrimmedString,
  STRIPE_PUBLISHABLE_KEY: optionalTrimmedString,
  STRIPE_WEBHOOK_SECRET: optionalTrimmedString,

  ORDER_TIMEOUT_MINUTES: z.string().default('5').transform(Number).pipe(z.number().int().positive()),
  MIN_RECHARGE_AMOUNT: z.string().default('1').transform(Number).pipe(z.number().positive()),
  MAX_RECHARGE_AMOUNT: z.string().default('1000').transform(Number).pipe(z.number().positive()),
  // 每日每用户最大累计充值额，0 = 不限制
  MAX_DAILY_RECHARGE_AMOUNT: z.string().default('10000').transform(Number).pipe(z.number().min(0)),

  // 每日各渠道全平台总限额，可选覆盖（0 = 不限制）。
  // 未设置时由各 PaymentProvider.defaultLimits 提供默认值。
  MAX_DAILY_AMOUNT_ALIPAY: z
    .string()
    .optional()
    .transform((v) => (v !== undefined ? Number(v) : undefined))
    .pipe(z.number().min(0).optional()),
  MAX_DAILY_AMOUNT_ALIPAY_DIRECT: z
    .string()
    .optional()
    .transform((v) => (v !== undefined ? Number(v) : undefined))
    .pipe(z.number().min(0).optional()),
  MAX_DAILY_AMOUNT_WXPAY: z
    .string()
    .optional()
    .transform((v) => (v !== undefined ? Number(v) : undefined))
    .pipe(z.number().min(0).optional()),
  MAX_DAILY_AMOUNT_STRIPE: z
    .string()
    .optional()
    .transform((v) => (v !== undefined ? Number(v) : undefined))
    .pipe(z.number().min(0).optional()),
  ADMIN_TOKEN: z.string().min(16),

  NEXT_PUBLIC_APP_URL: z.string().url(),
  PAY_HELP_IMAGE_URL: optionalTrimmedString,
  PAY_HELP_TEXT: optionalTrimmedString,

  // ── 支付方式前端描述（sublabel）覆盖，不设置则使用默认值 ──
  PAYMENT_SUBLABEL_ALIPAY: optionalTrimmedString,
  PAYMENT_SUBLABEL_ALIPAY_DIRECT: optionalTrimmedString,
  PAYMENT_SUBLABEL_WXPAY: optionalTrimmedString,
  PAYMENT_SUBLABEL_WXPAY_DIRECT: optionalTrimmedString,
  PAYMENT_SUBLABEL_STRIPE: optionalTrimmedString,
});

export type Env = z.infer<typeof envSchema>;

let cachedEnv: Env | null = null;

/**
 * 如果值看起来是文件路径且文件存在，则读取文件内容作为实际值；
 * 否则直接返回原值。
 */
function resolveKeyValue(value: string | undefined): string | undefined {
  if (!value) return undefined;
  // 密钥内容不会以 / 或盘符开头，文件路径才会
  if ((value.startsWith('/') || /^[A-Za-z]:[/\\]/.test(value)) && fs.existsSync(value)) {
    try {
      return fs.readFileSync(value, 'utf-8').trim();
    } catch (err) {
      throw new Error(`Failed to read key file ${value}: ${(err as Error).message}`);
    }
  }
  return value;
}

export function getEnv(): Env {
  if (cachedEnv) return cachedEnv;

  // 构建阶段不校验环境变量（next build 收集页面数据时会触发）
  if (process.env.NEXT_PHASE === 'phase-production-build') {
    return {} as Env;
  }

  const parsed = envSchema.safeParse(process.env);
  if (!parsed.success) {
    console.error('Invalid environment variables:', parsed.error.flatten().fieldErrors);
    throw new Error('Invalid environment variables');
  }

  const env = parsed.data;

  // 支付宝密钥：支持直接传内容或传文件路径
  env.ALIPAY_PRIVATE_KEY = resolveKeyValue(env.ALIPAY_PRIVATE_KEY);
  env.ALIPAY_PUBLIC_KEY = resolveKeyValue(env.ALIPAY_PUBLIC_KEY);

  // 微信支付密钥：支持直接传内容或传文件路径
  env.WXPAY_PRIVATE_KEY = resolveKeyValue(env.WXPAY_PRIVATE_KEY);
  env.WXPAY_PUBLIC_KEY = resolveKeyValue(env.WXPAY_PUBLIC_KEY);

  cachedEnv = env;
  return cachedEnv;
}
