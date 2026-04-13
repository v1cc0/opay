#!/usr/bin/env python3
"""Create an EasyPay order, emit a signed notify callback, and verify completion."""

from __future__ import annotations

import argparse
import hashlib
import json
import time
import urllib.error
import urllib.parse
import urllib.request
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


def request_text(url: str) -> tuple[int, str]:
    req = urllib.request.Request(url, headers={"Accept": "text/plain"})
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read().decode("utf-8")
            return resp.status, raw
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode("utf-8")


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def generate_easy_pay_sign(params: dict[str, str], pkey: str) -> str:
    filtered = sorted(
        (key, value)
        for key, value in params.items()
        if key not in {"sign", "sign_type"} and value
    )
    query = "&".join(f"{key}={value}" for key, value in filtered)
    sign_input = f"{query}{pkey}"
    return hashlib.md5(sign_input.encode("utf-8")).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description="EasyPay notify completion smoke")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--user-token", default="user-token")
    parser.add_argument("--payment-type", default="alipay")
    parser.add_argument("--amount", type=float, default=12.34)
    parser.add_argument("--lang", default="zh")
    parser.add_argument("--pid", default="easy_local_pid")
    parser.add_argument("--pkey", default="easy_local_pkey")
    parser.add_argument("--poll-timeout-seconds", type=int, default=15)
    parser.add_argument("--result-file", default="/tmp/opay-easypay-notify-completion-smoke.json")
    args = parser.parse_args()

    token_q = urllib.parse.quote(args.user_token)
    my_orders_url = f"{args.api_base}/api/orders/my?token={token_q}"
    status_code, my_orders_before = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders before create: {status_code} {my_orders_before}")
    initial_balance = float(my_orders_before["user"]["balance"])

    status_code, providers_payload = request_json(
        "GET",
        f"{args.api_base}/api/admin/provider-instances?token={urllib.parse.quote(args.admin_token)}",
    )
    expect(status_code == 200, f"failed to list provider instances: {status_code} {providers_payload}")
    easypay_instances = [
        item
        for item in providers_payload.get("instances", [])
        if item.get("providerKey") == "easypay" and item.get("enabled") is True
    ]
    expect(easypay_instances, "no enabled easypay provider instance found")
    instance_id = easypay_instances[0]["id"]

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
    expect(status_code == 200, f"create EasyPay order failed: {status_code} {create_payload}")

    order_id = create_payload["orderId"]
    status_access_token = create_payload["statusAccessToken"]
    pay_amount = float(create_payload["payAmount"])
    amount_cents = round(pay_amount * 100)

    status_code, detail_payload = request_json(
        "GET",
        f"{args.api_base}/api/admin/orders/{urllib.parse.quote(order_id)}?token={urllib.parse.quote(args.admin_token)}",
    )
    expect(status_code == 200, f"failed to fetch EasyPay admin detail: {status_code} {detail_payload}")
    trade_no = detail_payload.get("paymentTradeNo")
    expect(trade_no, "EasyPay admin detail missing paymentTradeNo")

    status_url = (
        f"{args.api_base}/api/orders/{urllib.parse.quote(order_id)}"
        f"?access_token={urllib.parse.quote(status_access_token)}"
    )
    status_code, order_status_before = request_json("GET", status_url)
    expect(status_code == 200, f"failed to fetch initial EasyPay order status: {status_code} {order_status_before}")
    expect(order_status_before["status"] == "PENDING", "new EasyPay order should start as PENDING")

    sign_params = {
        "pid": args.pid,
        "trade_no": trade_no,
        "out_trade_no": order_id,
        "money": f"{pay_amount:.2f}",
        "trade_status": "TRADE_SUCCESS",
    }
    sign = generate_easy_pay_sign(sign_params, args.pkey)
    notify_params = {
        **sign_params,
        "inst": instance_id,
    }
    notify_params["sign"] = sign
    notify_params["sign_type"] = "MD5"
    notify_query = urllib.parse.urlencode(notify_params)
    notify_url = f"{args.api_base}/api/easy-pay/notify?{notify_query}"
    notify_status, notify_body = request_text(notify_url)
    expect(notify_status == 200 and notify_body.strip() == "success", f"EasyPay notify failed: {notify_status} {notify_body}")

    deadline = time.time() + args.poll_timeout_seconds
    final_status = None
    while time.time() < deadline:
        status_code, polled = request_json("GET", status_url)
        expect(status_code == 200, f"status poll failed: {status_code} {polled}")
        if polled["status"] == "COMPLETED" and polled.get("rechargeSuccess") is True:
            final_status = polled
            break
        time.sleep(1)
    expect(final_status is not None, "EasyPay order did not reach COMPLETED within timeout")

    status_code, my_orders_after = request_json("GET", my_orders_url)
    expect(status_code == 200, f"failed to load my orders after notify: {status_code} {my_orders_after}")
    latest_order = next((item for item in my_orders_after["orders"] if item["id"] == order_id), None)
    expect(latest_order is not None, f"completed EasyPay order {order_id} missing from my orders")
    expect(latest_order["status"] == "COMPLETED", f"user EasyPay order status mismatch: {latest_order['status']}")

    final_balance = float(my_orders_after["user"]["balance"])
    expected_balance = round(initial_balance + args.amount, 2)
    expect(
        abs(final_balance - expected_balance) < 1e-6,
        f"user balance mismatch: expected {expected_balance}, got {final_balance}",
    )

    result = {
        "instanceId": instance_id,
        "orderId": order_id,
        "statusAccessToken": status_access_token,
        "initialBalance": initial_balance,
        "finalBalance": final_balance,
        "expectedBalance": expected_balance,
        "notifyResponse": notify_body.strip(),
        "status": final_status,
        "userOrder": latest_order,
        "createPayload": create_payload,
    }
    output = json.dumps(result, indent=2)
    with open(args.result_file, "w") as fh:
        fh.write(output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
