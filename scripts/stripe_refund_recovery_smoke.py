#!/usr/bin/env python3
"""Simulate gateway refund failure with rollback compensation, then retry refund successfully."""

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
) -> tuple[int, dict[str, Any]]:
    payload = None
    headers = {"Accept": "application/json"}
    if body is not None:
        payload = json.dumps(body, separators=(",", ":")).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=payload, method=method, headers=headers)
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
    parser = argparse.ArgumentParser(description="Stripe refund recovery smoke")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--platform-base", default="http://127.0.0.1:18080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--user-token", default="user-token")
    parser.add_argument("--amount", type=float, default=6.54)
    parser.add_argument("--result-file", default="/tmp/opay-stripe-refund-recovery-smoke.json")
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
    initial_balance = float(completed["initialBalance"])
    balance_after_completion = float(completed["finalBalance"])
    refund_amount = float(completed["userOrder"]["amount"])
    token_q = urllib.parse.quote(args.admin_token)

    status_code, control_payload = request_json(
        "POST",
        f"{args.platform_base}/__control/failures",
        {"fail_next_stripe_refund": 1},
    )
    expect(status_code == 200, f"failed to arm stripe refund failure: {status_code} {control_payload}")

    refund_request = {
        "order_id": order_id,
        "amount": refund_amount,
        "reason": "refund recovery smoke",
        "force": False,
        "deduct_balance": True,
    }
    refund_url = f"{args.api_base}/api/admin/refund?token={token_q}"
    status_code, refund_failed_payload = request_json("POST", refund_url, refund_request)
    expect(status_code == 200, f"refund failure path request failed: {status_code} {refund_failed_payload}")
    expect(refund_failed_payload.get("success") is False, f"expected rollback warning path: {refund_failed_payload}")
    warning = refund_failed_payload.get("warning", "")
    expect("回滚" in warning or "rolled back" in warning, f"unexpected refund warning: {warning}")

    detail_url = (
        f"{args.api_base}/api/admin/orders/{urllib.parse.quote(order_id)}"
        f"?token={token_q}"
    )
    status_code, detail_after_failure = request_json("GET", detail_url)
    expect(status_code == 200, f"failed to fetch admin detail after refund failure: {status_code} {detail_after_failure}")
    expect(detail_after_failure["status"] == "COMPLETED", "order should stay COMPLETED after rollback compensation")

    my_orders_url = f"{args.api_base}/api/orders/my?token={urllib.parse.quote(args.user_token)}"
    status_code, my_orders_after_failure = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to fetch user orders after refund failure: {status_code} {my_orders_after_failure}")
    balance_after_failure = float(my_orders_after_failure["user"]["balance"])
    expect(
        abs(balance_after_failure - balance_after_completion) < 1e-6,
        f"balance changed after rollback compensation: expected {balance_after_completion}, got {balance_after_failure}",
    )

    status_code, refund_success_payload = request_json("POST", refund_url, refund_request)
    expect(status_code == 200 and refund_success_payload.get("success") is True, f"refund retry failed: {status_code} {refund_success_payload}")

    status_code, detail_after_success = request_json("GET", detail_url)
    expect(status_code == 200, f"failed to fetch admin detail after refund success: {status_code} {detail_after_success}")
    expect(detail_after_success["status"] == "REFUNDED", f"refund retry should end in REFUNDED, got {detail_after_success['status']}")

    status_code, my_orders_after_success = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to fetch user orders after refund success: {status_code} {my_orders_after_success}")
    final_balance = float(my_orders_after_success["user"]["balance"])
    expect(
        abs(final_balance - initial_balance) < 1e-6,
        f"refund retry should restore balance to initial value {initial_balance}, got {final_balance}",
    )
    user_order = next((item for item in my_orders_after_success["orders"] if item["id"] == order_id), None)
    expect(user_order is not None, f"user order {order_id} missing after refund retry")
    expect(user_order["status"] == "REFUNDED", f"user order should be REFUNDED, got {user_order['status']}")

    actions = [item["action"] for item in detail_after_success.get("auditLogs", [])]
    audit_counts = {
        "REFUND_GATEWAY_FAILED": actions.count("REFUND_GATEWAY_FAILED"),
        "REFUND_FAILED": actions.count("REFUND_FAILED"),
        "REFUND_SUCCESS": actions.count("REFUND_SUCCESS"),
        "REFUND_ROLLBACK_FAILED": actions.count("REFUND_ROLLBACK_FAILED"),
    }
    expect(audit_counts["REFUND_GATEWAY_FAILED"] == 1, f"expected one REFUND_GATEWAY_FAILED, got {audit_counts}")
    expect(audit_counts["REFUND_FAILED"] == 0, f"expected no REFUND_FAILED after rollback compensation, got {audit_counts}")
    expect(audit_counts["REFUND_SUCCESS"] == 1, f"expected one REFUND_SUCCESS after retry, got {audit_counts}")
    expect(audit_counts["REFUND_ROLLBACK_FAILED"] == 0, f"did not expect REFUND_ROLLBACK_FAILED, got {audit_counts}")

    result = {
        "orderId": order_id,
        "initialBalance": initial_balance,
        "balanceAfterCompletion": balance_after_completion,
        "balanceAfterFailure": balance_after_failure,
        "finalBalance": final_balance,
        "refundWarning": warning,
        "detailAfterFailure": {
            "status": detail_after_failure["status"],
            "refundAmount": detail_after_failure.get("refundAmount"),
        },
        "detailAfterSuccess": {
            "status": detail_after_success["status"],
            "refundAmount": detail_after_success.get("refundAmount"),
        },
        "userOrder": user_order,
        "auditCounts": audit_counts,
    }
    output = json.dumps(result, indent=2)
    Path(args.result_file).write_text(output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
