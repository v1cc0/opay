#!/usr/bin/env python3
"""Simulate a paid Stripe order whose fulfillment fails once, then recover via admin retry."""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import time
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


def sign_stripe_payload(secret: str, payload: str, timestamp: int) -> str:
    signed_payload = f"{timestamp}.{payload}".encode("utf-8")
    digest = hmac.new(secret.encode("utf-8"), signed_payload, hashlib.sha256).hexdigest()
    return f"t={timestamp},v1={digest}"


def extract_payment_intent_id(client_secret: str) -> str:
    marker = "_secret_"
    expect(marker in client_secret, f"unexpected client_secret format: {client_secret}")
    return client_secret.split(marker, 1)[0]


def build_webhook_payload(order_id: str, payment_intent_id: str, amount_cents: int) -> str:
    return json.dumps(
        {
            "id": f"evt_recovery_{payment_intent_id}",
            "type": "payment_intent.succeeded",
            "data": {
                "object": {
                    "id": payment_intent_id,
                    "amount": amount_cents,
                    "metadata": {"orderId": order_id},
                }
            },
        },
        separators=(",", ":"),
    )


def poll_until(
    api_base: str,
    order_id: str,
    access_token: str,
    expected_status: str,
    timeout_seconds: int,
) -> dict[str, Any]:
    status_url = (
        f"{api_base}/api/orders/{urllib.parse.quote(order_id)}"
        f"?access_token={urllib.parse.quote(access_token)}"
    )
    deadline = time.time() + timeout_seconds
    latest = None
    while time.time() < deadline:
        status_code, payload = request_json("GET", status_url)
        expect(status_code == 200, f"status poll failed: {status_code} {payload}")
        latest = payload
        if payload["status"] == expected_status:
            return payload
        time.sleep(1)
    raise RuntimeError(f"order {order_id} did not reach {expected_status}: latest={latest}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Stripe recovery smoke")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--platform-base", default="http://127.0.0.1:18080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--user-token", default="user-token")
    parser.add_argument("--amount", type=float, default=7.89)
    parser.add_argument("--payment-type", default="stripe")
    parser.add_argument("--webhook-secret", default="whsec_local_smoke")
    parser.add_argument("--poll-timeout-seconds", type=int, default=15)
    parser.add_argument("--result-file", default="/tmp/opay-stripe-recovery-smoke.json")
    args = parser.parse_args()

    token_q = urllib.parse.quote(args.user_token)
    my_orders_url = f"{args.api_base}/api/orders/my?token={token_q}"
    status_code, my_orders_before = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders before create: {status_code} {my_orders_before}")
    initial_balance = float(my_orders_before["user"]["balance"])

    status_code, control_payload = request_json(
        "POST",
        f"{args.platform_base}/__control/failures",
        {"fail_next_balance_redeem": 1},
    )
    expect(status_code == 200, f"failed to configure mock failure: {status_code} {control_payload}")

    status_code, create_payload = request_json(
        "POST",
        f"{args.api_base}/api/orders",
        {
            "token": args.user_token,
            "amount": args.amount,
            "payment_type": args.payment_type,
            "is_mobile": False,
        },
    )
    expect(status_code == 200, f"create order failed: {status_code} {create_payload}")

    order_id = create_payload["orderId"]
    access_token = create_payload["statusAccessToken"]
    client_secret = create_payload["clientSecret"]
    payment_intent_id = extract_payment_intent_id(client_secret)
    amount_cents = round(float(create_payload["payAmount"]) * 100)

    webhook_payload = build_webhook_payload(order_id, payment_intent_id, amount_cents)
    signature = sign_stripe_payload(args.webhook_secret, webhook_payload, int(time.time()))
    status_code, webhook_response = request_json(
        "POST",
        f"{args.api_base}/api/stripe/webhook",
        json.loads(webhook_payload),
        headers={"Stripe-Signature": signature},
    )
    expect(status_code == 500, f"expected retryable webhook failure, got {status_code} {webhook_response}")

    failed_status = poll_until(
        args.api_base,
        order_id,
        access_token,
        "FAILED",
        args.poll_timeout_seconds,
    )
    expect(failed_status.get("paymentSuccess") is True, "failed fulfillment should still keep paymentSuccess=true")
    expect(failed_status.get("rechargeSuccess") is False, "failed fulfillment should not mark rechargeSuccess")

    status_code, my_orders_failed = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders after failed fulfillment: {status_code} {my_orders_failed}")
    balance_after_failure = float(my_orders_failed["user"]["balance"])
    expect(
        abs(balance_after_failure - initial_balance) < 1e-6,
        f"balance changed after failed fulfillment: expected {initial_balance}, got {balance_after_failure}",
    )

    retry_status_code, retry_payload = request_json(
        "POST",
        f"{args.api_base}/api/admin/orders/{urllib.parse.quote(order_id)}/retry?token={urllib.parse.quote(args.admin_token)}",
    )
    expect(retry_status_code == 200 and retry_payload.get("success") is True, f"retry failed: {retry_status_code} {retry_payload}")

    completed_status = poll_until(
        args.api_base,
        order_id,
        access_token,
        "COMPLETED",
        args.poll_timeout_seconds,
    )
    expect(completed_status.get("rechargeSuccess") is True, "retry should eventually complete fulfillment")

    status_code, my_orders_after = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders after retry: {status_code} {my_orders_after}")
    final_balance = float(my_orders_after["user"]["balance"])
    expected_balance = round(initial_balance + args.amount, 2)
    expect(
        abs(final_balance - expected_balance) < 1e-6,
        f"final balance mismatch: expected {expected_balance}, got {final_balance}",
    )
    final_user_order = next((item for item in my_orders_after["orders"] if item["id"] == order_id), None)
    expect(final_user_order is not None, f"order {order_id} missing from my orders after retry")
    expect(final_user_order["status"] == "COMPLETED", f"user order did not recover to COMPLETED: {final_user_order}")

    detail_url = (
        f"{args.api_base}/api/admin/orders/{urllib.parse.quote(order_id)}"
        f"?token={urllib.parse.quote(args.admin_token)}"
    )
    status_code, detail_payload = request_json("GET", detail_url)
    expect(status_code == 200, f"failed to fetch admin detail after retry: {status_code} {detail_payload}")
    actions = [item["action"] for item in detail_payload.get("auditLogs", [])]
    expect(actions.count("ORDER_PAID") == 1, "recovery flow should only record ORDER_PAID once")
    expect(actions.count("RECHARGE_FAILED") == 1, "recovery flow should record one RECHARGE_FAILED")
    expect(actions.count("RECHARGE_RETRY") == 1, "recovery flow should record one RECHARGE_RETRY")
    expect(actions.count("RECHARGE_SUCCESS") == 1, "recovery flow should record one RECHARGE_SUCCESS")

    result = {
        "orderId": order_id,
        "statusAccessToken": access_token,
        "clientSecret": client_secret,
        "paymentIntentId": payment_intent_id,
        "initialBalance": initial_balance,
        "balanceAfterFailure": balance_after_failure,
        "finalBalance": final_balance,
        "expectedBalance": expected_balance,
        "webhookResponse": webhook_response,
        "failedStatus": failed_status,
        "completedStatus": completed_status,
        "userOrder": final_user_order,
        "auditCounts": {
            "ORDER_PAID": actions.count("ORDER_PAID"),
            "RECHARGE_FAILED": actions.count("RECHARGE_FAILED"),
            "RECHARGE_RETRY": actions.count("RECHARGE_RETRY"),
            "RECHARGE_SUCCESS": actions.count("RECHARGE_SUCCESS"),
        },
    }
    output = json.dumps(result, indent=2)
    Path(args.result_file).write_text(output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
