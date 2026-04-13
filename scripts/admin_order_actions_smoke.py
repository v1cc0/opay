#!/usr/bin/env python3
"""Seed admin smoke orders and validate admin order actions end-to-end.

This script assumes the local Rust backend and platform mock are already running.
It uses only Python stdlib.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class SeededOrders:
    cancel_order_id: str
    retry_order_id: str
    refund_order_id: str


def unix_now() -> int:
    return int(time.time())


def generate_recharge_code(label: str) -> str:
    return f"SMOKE-{label.upper()}-{uuid.uuid4().hex[:12].upper()}"


def request_json(method: str, url: str, body: dict[str, Any] | None = None) -> tuple[int, dict[str, Any]]:
    data = None
    headers = {"Accept": "application/json"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read().decode("utf-8")
            return resp.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8")
        payload = json.loads(raw) if raw else {}
        return exc.code, payload


def delete_previous_seed_data(conn: sqlite3.Connection) -> None:
    order_ids = [
        row[0]
        for row in conn.execute(
            "SELECT id FROM orders WHERE src_host = 'admin-smoke'"
        ).fetchall()
    ]
    if order_ids:
        conn.executemany("DELETE FROM audit_logs WHERE order_id = ?", [(order_id,) for order_id in order_ids])
    conn.execute("DELETE FROM orders WHERE src_host = 'admin-smoke'")
    conn.commit()


def seed_orders(db_path: Path, user_id: int) -> SeededOrders:
    conn = sqlite3.connect(db_path, timeout=30)
    try:
        conn.execute("PRAGMA busy_timeout = 30000")
        delete_previous_seed_data(conn)
        now = unix_now()

        cancel_order_id = str(uuid.uuid4())
        retry_order_id = str(uuid.uuid4())
        refund_order_id = str(uuid.uuid4())

        orders = [
            {
                "id": cancel_order_id,
                "user_id": user_id,
                "amount_cents": 2100,
                "pay_amount_cents": 2100,
                "status": "PENDING",
                "payment_type": "stripe",
                "payment_trade_no": None,
                "expires_at": now + 600,
                "paid_at": None,
                "completed_at": None,
                "failed_at": None,
                "failed_reason": None,
                "order_type": "balance",
                "provider_instance_id": None,
                "created_at": now - 3,
                "updated_at": now - 3,
                "recharge_code": generate_recharge_code("cancel"),
            },
            {
                "id": retry_order_id,
                "user_id": user_id,
                "amount_cents": 3200,
                "pay_amount_cents": 3200,
                "status": "FAILED",
                "payment_type": "stripe",
                "payment_trade_no": "pi_smoke_retry_local",
                "expires_at": now - 500,
                "paid_at": now - 480,
                "completed_at": None,
                "failed_at": now - 470,
                "failed_reason": "mock recharge failure",
                "order_type": "balance",
                "provider_instance_id": None,
                "created_at": now - 20,
                "updated_at": now - 20,
                "recharge_code": generate_recharge_code("retry"),
            },
            {
                "id": refund_order_id,
                "user_id": user_id,
                "amount_cents": 4500,
                "pay_amount_cents": 4500,
                "status": "COMPLETED",
                "payment_type": "stripe",
                "payment_trade_no": None,
                "expires_at": now - 1000,
                "paid_at": now - 980,
                "completed_at": now - 970,
                "failed_at": None,
                "failed_reason": None,
                "order_type": "balance",
                "provider_instance_id": None,
                "created_at": now - 40,
                "updated_at": now - 40,
                "recharge_code": generate_recharge_code("refund"),
            },
        ]

        for order in orders:
            conn.execute(
                """
                INSERT INTO orders (
                  id, user_id, amount_cents, pay_amount_cents, fee_rate_bps, recharge_code,
                  status, payment_type, payment_trade_no, expires_at, paid_at, completed_at,
                  failed_at, failed_reason, created_at, updated_at, src_host, order_type,
                  provider_instance_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    order["id"],
                    order["user_id"],
                    order["amount_cents"],
                    order["pay_amount_cents"],
                    0,
                    order["recharge_code"],
                    order["status"],
                    order["payment_type"],
                    order["payment_trade_no"],
                    order["expires_at"],
                    order["paid_at"],
                    order["completed_at"],
                    order["failed_at"],
                    order["failed_reason"],
                    order["created_at"],
                    order["updated_at"],
                    "admin-smoke",
                    order["order_type"],
                    order["provider_instance_id"],
                ),
            )
            conn.execute(
                """
                INSERT INTO audit_logs (id, order_id, action, detail, operator, created_at)
                VALUES (?, ?, ?, ?, ?, ?)
                """,
                (
                    str(uuid.uuid4()),
                    order["id"],
                    "ORDER_CREATED",
                    json.dumps(
                        {
                            "seed": "admin-smoke",
                            "status": order["status"],
                            "amountCents": order["amount_cents"],
                        }
                    ),
                    "seed:admin-smoke",
                    order["created_at"],
                ),
            )

        conn.commit()
        return SeededOrders(
            cancel_order_id=cancel_order_id,
            retry_order_id=retry_order_id,
            refund_order_id=refund_order_id,
        )
    finally:
        conn.close()


def seeded_orders_to_dict(seeded: SeededOrders) -> dict[str, str]:
    return {
        "cancel": seeded.cancel_order_id,
        "retry": seeded.retry_order_id,
        "refund": seeded.refund_order_id,
    }


def load_seeded_orders(ids_file: Path) -> SeededOrders:
    payload = json.loads(ids_file.read_text())
    data = payload.get("seededOrders", payload)
    return SeededOrders(
        cancel_order_id=data["cancel"],
        retry_order_id=data["retry"],
        refund_order_id=data["refund"],
    )


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def admin_url(api_base: str, path: str, token: str) -> str:
    sep = "&" if "?" in path else "?"
    return f"{api_base}{path}{sep}token={urllib.parse.quote(token)}"


def get_order_detail(api_base: str, admin_token: str, order_id: str) -> dict[str, Any]:
    status, payload = request_json("GET", admin_url(api_base, f"/api/admin/orders/{order_id}", admin_token))
    expect(status == 200, f"admin detail failed for {order_id}: {status} {payload}")
    return payload


def list_my_orders(api_base: str, user_token: str) -> dict[str, Any]:
    status, payload = request_json(
        "GET",
        f"{api_base}/api/orders/my?token={urllib.parse.quote(user_token)}",
    )
    expect(status == 200, f"user orders failed: {status} {payload}")
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description="Admin order actions smoke")
    parser.add_argument("--db-path", default="data/opay-smoke.db")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--user-token", default="user-token")
    parser.add_argument("--user-id", type=int, default=42)
    parser.add_argument("--refund-amount", type=float, default=5.0)
    parser.add_argument("--ids-file", default="/tmp/opay-admin-order-actions-ids.json")
    parser.add_argument("--seed-only", action="store_true")
    parser.add_argument("--skip-seed", action="store_true")
    args = parser.parse_args()

    db_path = Path(args.db_path)
    expect(db_path.exists(), f"db path not found: {db_path}")
    ids_file = Path(args.ids_file)

    if args.seed_only and args.skip_seed:
        raise RuntimeError("--seed-only and --skip-seed are mutually exclusive")

    if args.skip_seed:
        expect(ids_file.exists(), f"ids file not found: {ids_file}")
        seeded = load_seeded_orders(ids_file)
    else:
        seeded = seed_orders(db_path, args.user_id)
        ids_payload = {"seededOrders": seeded_orders_to_dict(seeded)}
        ids_file.write_text(json.dumps(ids_payload, indent=2))
        if args.seed_only:
            print(json.dumps(ids_payload, indent=2))
            return 0

    list_status, list_payload = request_json(
        "GET",
        admin_url(args.api_base, f"/api/admin/orders?user_id={args.user_id}&page=1&page_size=20", args.admin_token),
    )
    expect(list_status == 200, f"admin order list failed: {list_status} {list_payload}")
    listed_ids = {item["id"] for item in list_payload.get("orders", [])}
    expect(seeded.cancel_order_id in listed_ids, "seeded cancel order missing from admin list")
    expect(seeded.retry_order_id in listed_ids, "seeded retry order missing from admin list")
    expect(seeded.refund_order_id in listed_ids, "seeded refund order missing from admin list")

    before_retry_detail = get_order_detail(args.api_base, args.admin_token, seeded.retry_order_id)
    expect(before_retry_detail["status"] == "FAILED", "retry seed order should start as FAILED")

    cancel_status, cancel_payload = request_json(
        "POST",
        admin_url(args.api_base, f"/api/admin/orders/{seeded.cancel_order_id}/cancel", args.admin_token),
    )
    expect(cancel_status == 200 and cancel_payload.get("success") is True, f"cancel failed: {cancel_status} {cancel_payload}")
    cancel_detail = get_order_detail(args.api_base, args.admin_token, seeded.cancel_order_id)
    expect(cancel_detail["status"] == "CANCELLED", f"cancel detail mismatch: {cancel_detail['status']}")

    retry_status, retry_payload = request_json(
        "POST",
        admin_url(args.api_base, f"/api/admin/orders/{seeded.retry_order_id}/retry", args.admin_token),
    )
    expect(retry_status == 200 and retry_payload.get("success") is True, f"retry failed: {retry_status} {retry_payload}")
    retry_detail = get_order_detail(args.api_base, args.admin_token, seeded.retry_order_id)
    expect(retry_detail["status"] == "COMPLETED", f"retry detail mismatch: {retry_detail['status']}")
    expect(retry_detail.get("rechargeSuccess") is True, "retry should make rechargeSuccess true")

    refund_status, refund_payload = request_json(
        "POST",
        admin_url(args.api_base, "/api/admin/refund", args.admin_token),
        {
            "order_id": seeded.refund_order_id,
            "amount": args.refund_amount,
            "reason": "admin smoke partial refund",
            "force": False,
            "deduct_balance": True,
        },
    )
    expect(refund_status == 200 and refund_payload.get("success") is True, f"refund failed: {refund_status} {refund_payload}")
    refund_detail = get_order_detail(args.api_base, args.admin_token, seeded.refund_order_id)
    expect(
        refund_detail["status"] == "PARTIALLY_REFUNDED",
        f"refund detail mismatch: {refund_detail['status']}",
    )
    expect(abs((refund_detail.get("refundAmount") or 0) - args.refund_amount) < 1e-6, "refund amount mismatch")

    my_orders = list_my_orders(args.api_base, args.user_token)
    user_status = {item["id"]: item["status"] for item in my_orders.get("orders", [])}
    expect(user_status.get(seeded.cancel_order_id) == "CANCELLED", "user order status mismatch for cancel order")
    expect(user_status.get(seeded.retry_order_id) == "COMPLETED", "user order status mismatch for retry order")
    expect(
        user_status.get(seeded.refund_order_id) == "PARTIALLY_REFUNDED",
        "user order status mismatch for refund order",
    )

    result = {
        "seededOrders": seeded_orders_to_dict(seeded),
        "adminResults": {
            "cancelStatus": cancel_detail["status"],
            "retryStatus": retry_detail["status"],
            "refundStatus": refund_detail["status"],
            "refundAmount": refund_detail.get("refundAmount"),
        },
        "userResults": {
            "cancelStatus": user_status.get(seeded.cancel_order_id),
            "retryStatus": user_status.get(seeded.retry_order_id),
            "refundStatus": user_status.get(seeded.refund_order_id),
        },
    }
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
