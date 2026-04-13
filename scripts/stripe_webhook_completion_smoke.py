#!/usr/bin/env python3
"""Create a Stripe order, emit a signed webhook, and verify fulfillment completion."""

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
            "id": f"evt_local_{payment_intent_id}",
            "type": "payment_intent.succeeded",
            "data": {
                "object": {
                    "id": payment_intent_id,
                    "amount": amount_cents,
                    "metadata": {
                        "orderId": order_id,
                    },
                }
            },
        },
        separators=(",", ":"),
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Stripe webhook completion smoke")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--user-token", default="user-token")
    parser.add_argument("--payment-type", default="stripe")
    parser.add_argument("--amount", type=float, default=20.0)
    parser.add_argument("--webhook-secret", default="whsec_local_smoke")
    parser.add_argument("--lang", default="zh")
    parser.add_argument("--poll-timeout-seconds", type=int, default=15)
    parser.add_argument("--result-file", default="/tmp/opay-stripe-webhook-completion-smoke.json")
    args = parser.parse_args()

    encoded_token = urllib.parse.quote(args.user_token)
    my_orders_url = f"{args.api_base}/api/orders/my?token={encoded_token}"
    status_code, my_orders_before = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders before create: {status_code} {my_orders_before}")
    initial_balance = float(my_orders_before["user"]["balance"])

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
    status_access_token = create_payload["statusAccessToken"]
    client_secret = create_payload["clientSecret"]
    pay_amount = float(create_payload["payAmount"])
    amount_cents = round(pay_amount * 100)
    payment_intent_id = extract_payment_intent_id(client_secret)

    status_url = (
        f"{args.api_base}/api/orders/{urllib.parse.quote(order_id)}"
        f"?access_token={urllib.parse.quote(status_access_token)}"
    )
    status_code, order_status_before = request_json("GET", status_url)
    expect(status_code == 200, f"failed to fetch initial order status: {status_code} {order_status_before}")
    expect(order_status_before["status"] == "PENDING", "new order should start as PENDING")

    webhook_payload = build_webhook_payload(order_id, payment_intent_id, amount_cents)
    timestamp = int(time.time())
    signature = sign_stripe_payload(args.webhook_secret, webhook_payload, timestamp)
    status_code, webhook_response = request_json(
        "POST",
        f"{args.api_base}/api/stripe/webhook",
        json.loads(webhook_payload),
        headers={"Stripe-Signature": signature},
    )
    expect(status_code == 200, f"webhook failed: {status_code} {webhook_response}")

    deadline = time.time() + args.poll_timeout_seconds
    final_status = None
    while time.time() < deadline:
        status_code, polled = request_json("GET", status_url)
        expect(status_code == 200, f"status poll failed: {status_code} {polled}")
        if polled["status"] == "COMPLETED" and polled.get("rechargeSuccess") is True:
            final_status = polled
            break
        time.sleep(1)
    expect(final_status is not None, "order did not reach COMPLETED within timeout")

    status_code, my_orders_after = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders after webhook: {status_code} {my_orders_after}")
    latest_order = next((item for item in my_orders_after["orders"] if item["id"] == order_id), None)
    expect(latest_order is not None, f"completed order {order_id} missing from my orders")
    expect(latest_order["status"] == "COMPLETED", f"user order status mismatch: {latest_order['status']}")

    final_balance = float(my_orders_after["user"]["balance"])
    expected_balance = round(initial_balance + args.amount, 2)
    expect(
        abs(final_balance - expected_balance) < 1e-6,
        f"user balance mismatch: expected {expected_balance}, got {final_balance}",
    )

    result = {
        "orderId": order_id,
        "statusAccessToken": status_access_token,
        "clientSecret": client_secret,
        "paymentIntentId": payment_intent_id,
        "initialBalance": initial_balance,
        "finalBalance": final_balance,
        "expectedBalance": expected_balance,
        "status": final_status,
        "userOrder": latest_order,
    }

    output = json.dumps(result, indent=2)
    Path(args.result_file).write_text(output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
