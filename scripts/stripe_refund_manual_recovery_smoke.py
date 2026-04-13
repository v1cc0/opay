#!/usr/bin/env python3
"""Simulate refund rollback failure, then manually compensate balance and retry refund."""

from __future__ import annotations

import argparse
import json
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


def request_json(
    method: str,
    url: str,
    body: dict[str, Any] | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, Any]]:
    payload = None
    req_headers = {"Accept": "application/json"}
    if headers:
        req_headers.update(headers)
    if body is not None:
        payload = json.dumps(body, separators=(",", ":")).encode("utf-8")
        req_headers.setdefault("Content-Type", "application/json")
    req = urllib.request.Request(url, data=payload, method=method, headers=req_headers)
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read().decode("utf-8")
            return resp.status, json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8")
        return exc.code, json.loads(raw) if raw else {}


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def run_capture(cmd: list[str]) -> dict[str, Any]:
    result = subprocess.run(cmd, check=True, text=True, capture_output=True)
    return json.loads(result.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description="Stripe refund manual recovery smoke")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--platform-base", default="http://127.0.0.1:18080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--user-token", default="user-token")
    parser.add_argument("--amount", type=float, default=4.32)
    parser.add_argument("--result-file", default="/tmp/opay-stripe-refund-manual-recovery-smoke.json")
    args = parser.parse_args()

    completed = run_capture(
        [
            "python3",
            "scripts/stripe_webhook_completion_smoke.py",
            "--api-base",
            args.api_base,
            "--admin-token",
            args.admin_token,
            "--user-token",
            args.user_token,
            "--amount",
            str(args.amount),
        ]
    )
    order_id = completed["orderId"]
    order_amount = float(completed["userOrder"]["amount"])
    initial_balance = float(completed["initialBalance"])
    balance_after_completion = float(completed["finalBalance"])

    status_code, control_payload = request_json(
        "POST",
        f"{args.platform_base}/__control/failures",
        {
            "fail_next_stripe_refund": 1,
            "fail_next_balance_add": 1,
        },
    )
    expect(status_code == 200, f"failed to arm refund failure controls: {status_code} {control_payload}")

    refund_url = f"{args.api_base}/api/admin/refund?token={urllib.parse.quote(args.admin_token)}"
    refund_request = {
        "order_id": order_id,
        "amount": order_amount,
        "reason": "manual refund recovery smoke",
        "force": False,
        "deduct_balance": True,
    }
    status_code, failure_payload = request_json("POST", refund_url, refund_request)
    expect(status_code == 500, f"expected hard refund failure, got {status_code} {failure_payload}")

    detail_url = (
        f"{args.api_base}/api/admin/orders/{urllib.parse.quote(order_id)}"
        f"?token={urllib.parse.quote(args.admin_token)}"
    )
    status_code, detail_after_failure = request_json("GET", detail_url)
    expect(status_code == 200, f"failed to fetch detail after hard refund failure: {status_code} {detail_after_failure}")
    expect(detail_after_failure["status"] == "REFUND_FAILED", f"expected REFUND_FAILED, got {detail_after_failure['status']}")

    my_orders_url = f"{args.api_base}/api/orders/my?token={urllib.parse.quote(args.user_token)}"
    status_code, my_orders_after_failure = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to fetch my orders after hard refund failure: {status_code} {my_orders_after_failure}")
    balance_after_failure = float(my_orders_after_failure["user"]["balance"])
    expected_after_failure = round(balance_after_completion - order_amount, 2)
    expect(
        abs(balance_after_failure - initial_balance) < 1e-6,
        f"balance after hard refund failure mismatch: expected {initial_balance}, got {balance_after_failure}",
    )

    retry_request = {
        "order_id": order_id,
        "amount": order_amount,
        "reason": "manual refund recovery retry",
        "force": False,
        "deduct_balance": False,
    }
    status_code, retry_payload = request_json("POST", refund_url, retry_request)
    expect(status_code == 200 and retry_payload.get("success") is True, f"refund retry failed: {status_code} {retry_payload}")

    status_code, detail_after_retry = request_json("GET", detail_url)
    expect(status_code == 200, f"failed to fetch detail after manual recovery retry: {status_code} {detail_after_retry}")
    expect(detail_after_retry["status"] == "REFUNDED", f"expected REFUNDED, got {detail_after_retry['status']}")

    status_code, my_orders_after_retry = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to fetch my orders after manual recovery retry: {status_code} {my_orders_after_retry}")
    final_balance = float(my_orders_after_retry["user"]["balance"])
    expect(
        abs(final_balance - initial_balance) < 1e-6,
        f"final balance mismatch after manual recovery: expected {initial_balance}, got {final_balance}",
    )
    final_user_order = next((item for item in my_orders_after_retry["orders"] if item["id"] == order_id), None)
    expect(final_user_order is not None, f"user order {order_id} missing after manual recovery")
    expect(final_user_order["status"] == "REFUNDED", f"user order should be REFUNDED, got {final_user_order['status']}")

    actions = [item["action"] for item in detail_after_retry.get("auditLogs", [])]
    audit_counts = {
        "REFUND_GATEWAY_FAILED": actions.count("REFUND_GATEWAY_FAILED"),
        "REFUND_FAILED": actions.count("REFUND_FAILED"),
        "REFUND_SUCCESS": actions.count("REFUND_SUCCESS"),
        "REFUND_ROLLBACK_FAILED": actions.count("REFUND_ROLLBACK_FAILED"),
    }
    expect(audit_counts["REFUND_GATEWAY_FAILED"] == 0, f"did not expect REFUND_GATEWAY_FAILED in hard failure path: {audit_counts}")
    expect(audit_counts["REFUND_FAILED"] == 1, f"expected one REFUND_FAILED, got {audit_counts}")
    expect(audit_counts["REFUND_SUCCESS"] == 1, f"expected one REFUND_SUCCESS after manual retry, got {audit_counts}")
    expect(audit_counts["REFUND_ROLLBACK_FAILED"] == 1, f"expected one REFUND_ROLLBACK_FAILED, got {audit_counts}")

    result = {
        "orderId": order_id,
        "initialBalance": initial_balance,
        "balanceAfterCompletion": balance_after_completion,
        "balanceAfterFailure": balance_after_failure,
        "finalBalance": final_balance,
        "detailAfterFailure": {
            "status": detail_after_failure["status"],
            "failedReason": detail_after_failure.get("failedReason"),
        },
        "detailAfterRetry": {
            "status": detail_after_retry["status"],
            "refundAmount": detail_after_retry.get("refundAmount"),
        },
        "userOrder": final_user_order,
        "auditCounts": audit_counts,
    }
    output = json.dumps(result, indent=2)
    Path(args.result_file).write_text(output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
