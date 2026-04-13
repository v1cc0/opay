export interface Sub2ApiUser {
  id: number;
  username: string;
  email: string;
  status: string; // "active", "banned", etc.
  balance: number;
  notes?: string;
}

export interface Sub2ApiRedeemCode {
  id: number;
  code: string;
  type: string;
  value: number;
  status: string;
  used_by: number;
  used_at: string;
}

export interface Sub2ApiResponse<T> {
  code: number;
  data?: T;
  message?: string;
}

// ── 分组 ──

export interface Sub2ApiGroup {
  id: number;
  name: string;
  description: string;
  platform: string;
  status: string;
  rate_multiplier: number;
  subscription_type: string; // "standard" | "subscription"
  daily_limit_usd: number | null;
  weekly_limit_usd: number | null;
  monthly_limit_usd: number | null;
  default_validity_days: number;
  sort_order: number;
  supported_model_scopes: string[] | null;
  allow_messages_dispatch?: boolean;
  default_mapped_model?: string;
}

// ── 订阅 ──

export interface Sub2ApiSubscription {
  id: number;
  user_id: number;
  group_id: number;
  starts_at: string;
  expires_at: string;
  status: string; // "active" | "expired" | "suspended"
  daily_usage_usd: number;
  weekly_usage_usd: number;
  monthly_usage_usd: number;
  daily_window_start: string | null;
  weekly_window_start: string | null;
  monthly_window_start: string | null;
  assigned_by: number;
  assigned_at: string;
  notes: string | null;
  created_at: string;
  updated_at: string;
}
