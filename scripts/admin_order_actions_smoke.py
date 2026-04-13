#!/usr/bin/env python3
"""Seed admin smoke orders and validate admin order actions end-to-end.

This script assumes the local Rust backend and platform mock are already running.
It uses only Python stdlib.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class SeededOrders:
    cancel_order_id: str
    retry_order_id: str
    refund_order_id: str


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


def seed_orders(db_path: Path, user_id: int) -> SeededOrders:
    result = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--example",
            "seed_admin_orders",
            "--",
            "--db-path",
            str(db_path),
            "--user-id",
            str(user_id),
        ],
        check=True,
        text=True,
        capture_output=True,
    )
    payload = json.loads(result.stdout)
    data = payload["seededOrders"]
    return SeededOrders(
        cancel_order_id=data["cancel"],
        retry_order_id=data["retry"],
        refund_order_id=data["refund"],
    )


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
