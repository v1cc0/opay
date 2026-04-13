#!/usr/bin/env python3
"""Ensure a local EasyPay provider instance exists and enable alipay locally."""

from __future__ import annotations

import argparse
import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


def request_json(method: str, url: str, body: dict[str, Any] | None = None) -> tuple[int, dict[str, Any]]:
    payload = None
    headers = {"Accept": "application/json"}
    if body is not None:
        payload = json.dumps(body).encode("utf-8")
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


def main() -> int:
    parser = argparse.ArgumentParser(description="Configure local EasyPay mock provider instance")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--instance-name", default="EasyPay Local Smoke")
    parser.add_argument("--payment-api-base", default="http://127.0.0.1:18081")
    parser.add_argument("--pid", default="easy_local_pid")
    parser.add_argument("--pkey", default="easy_local_pkey")
    parser.add_argument("--cid-alipay", default="easy_local_cid_alipay")
    parser.add_argument("--cid-wxpay", default="easy_local_cid_wxpay")
    args = parser.parse_args()

    token_q = urllib.parse.quote(args.admin_token)
    instances_url = f"{args.api_base}/api/admin/provider-instances?token={token_q}"
    status, payload = request_json("GET", instances_url)
    expect(status == 200, f"failed to list provider instances: {status} {payload}")

    easypay_instances = [
        item
        for item in payload.get("instances", [])
        if item.get("providerKey") == "easypay"
    ]

    config = {
        "pid": args.pid,
        "pkey": args.pkey,
        "apiBase": args.payment_api_base,
        "notifyUrl": f"{args.api_base}/api/easy-pay/notify",
        "returnUrl": f"{args.api_base}/pay/result",
        "cidAlipay": args.cid_alipay,
        "cidWxpay": args.cid_wxpay,
    }

    if easypay_instances:
        instance_id = easypay_instances[0]["id"]
        status, payload = request_json(
            "PUT",
            f"{args.api_base}/api/admin/provider-instances/{urllib.parse.quote(instance_id)}?token={token_q}",
            {
                "name": args.instance_name,
                "enabled": True,
                "supportedTypes": "alipay,wxpay",
                "refundEnabled": True,
                "config": config,
            },
        )
        expect(status == 200, f"failed to update easypay instance: {status} {payload}")
    else:
        status, payload = request_json(
            "POST",
            instances_url,
            {
                "providerKey": "easypay",
                "name": args.instance_name,
                "config": config,
                "enabled": True,
                "sortOrder": 1,
                "supportedTypes": "alipay,wxpay",
                "refundEnabled": True,
            },
        )
        expect(status == 201, f"failed to create easypay instance: {status} {payload}")
        instance_id = payload["id"]

    status, payload = request_json(
        "PUT",
        f"{args.api_base}/api/admin/config?token={token_q}",
        {
            "configs": [
                {"key": "OVERRIDE_ENV_ENABLED", "value": "true", "group": "payment", "label": "Override env config"},
                {"key": "ENABLED_PROVIDERS", "value": "stripe,easypay", "group": "payment", "label": "Enabled providers"},
                {"key": "ENABLED_PAYMENT_TYPES", "value": "stripe,alipay", "group": "payment", "label": "Enabled payment types"},
            ]
        },
    )
    expect(status == 200, f"failed to update payment config: {status} {payload}")

    print(
        json.dumps(
            {
                "instanceId": instance_id,
                "providerKey": "easypay",
                "apiBase": args.payment_api_base,
                "pid": args.pid,
                "pkey": args.pkey,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
