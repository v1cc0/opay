CREATE TABLE IF NOT EXISTS orders (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    user_email TEXT,
    user_name TEXT,
    user_notes TEXT,
    amount_cents INTEGER NOT NULL,
    pay_amount_cents INTEGER,
    fee_rate_bps INTEGER,
    recharge_code TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    payment_type TEXT NOT NULL,
    payment_trade_no TEXT,
    pay_url TEXT,
    qr_code TEXT,
    qr_code_img TEXT,
    refund_amount_cents INTEGER,
    refund_reason TEXT,
    refund_at INTEGER,
    force_refund INTEGER NOT NULL DEFAULT 0,
    refund_requested_at INTEGER,
    refund_request_reason TEXT,
    refund_requested_by INTEGER,
    expires_at INTEGER NOT NULL,
    paid_at INTEGER,
    completed_at INTEGER,
    failed_at INTEGER,
    failed_reason TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    client_ip TEXT,
    src_host TEXT,
    src_url TEXT,
    order_type TEXT NOT NULL DEFAULT 'balance',
    plan_id TEXT,
    subscription_group_id INTEGER,
    subscription_days INTEGER,
    provider_instance_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_orders_user_id ON orders(user_id);
CREATE INDEX IF NOT EXISTS idx_orders_status ON orders(status);
CREATE INDEX IF NOT EXISTS idx_orders_expires_at ON orders(expires_at);
CREATE INDEX IF NOT EXISTS idx_orders_created_at ON orders(created_at);
CREATE INDEX IF NOT EXISTS idx_orders_paid_at ON orders(paid_at);
CREATE INDEX IF NOT EXISTS idx_orders_payment_type_paid_at ON orders(payment_type, paid_at);
CREATE INDEX IF NOT EXISTS idx_orders_order_type ON orders(order_type);
CREATE INDEX IF NOT EXISTS idx_orders_provider_instance_id ON orders(provider_instance_id);

CREATE TABLE IF NOT EXISTS audit_logs (
    id TEXT PRIMARY KEY,
    order_id TEXT NOT NULL,
    action TEXT NOT NULL,
    detail TEXT,
    operator TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY(order_id) REFERENCES orders(id)
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_order_id ON audit_logs(order_id);
CREATE INDEX IF NOT EXISTS idx_audit_logs_created_at ON audit_logs(created_at);

CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    group_id INTEGER UNIQUE,
    name TEXT NOT NULL,
    platform TEXT NOT NULL DEFAULT 'claude',
    rate_multiplier_bps INTEGER NOT NULL DEFAULT 10000,
    description TEXT,
    models TEXT,
    features TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_channels_sort_order ON channels(sort_order);

CREATE TABLE IF NOT EXISTS subscription_plans (
    id TEXT PRIMARY KEY,
    group_id INTEGER,
    name TEXT NOT NULL,
    description TEXT,
    price_cents INTEGER NOT NULL,
    original_price_cents INTEGER,
    validity_days INTEGER NOT NULL DEFAULT 30,
    validity_unit TEXT NOT NULL DEFAULT 'day',
    features TEXT,
    product_name TEXT,
    for_sale INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_subscription_plans_for_sale_sort_order
    ON subscription_plans(for_sale, sort_order);

CREATE TABLE IF NOT EXISTS payment_provider_instances (
    id TEXT PRIMARY KEY,
    provider_key TEXT NOT NULL,
    name TEXT NOT NULL,
    config TEXT NOT NULL,
    supported_types TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    sort_order INTEGER NOT NULL DEFAULT 0,
    limits TEXT,
    refund_enabled INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_payment_provider_instances_provider_key
    ON payment_provider_instances(provider_key);
CREATE INDEX IF NOT EXISTS idx_payment_provider_instances_provider_key_enabled
    ON payment_provider_instances(provider_key, enabled);
