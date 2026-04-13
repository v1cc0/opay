#!/usr/bin/env python3
"""Point the local Stripe provider instance at the local mock API."""

from __future__ import annotations

import argparse
import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


def request_json(
    method: str,
    url: str,
    body: dict[str, Any] | None = None,
) -> tuple[int, dict[str, Any]]:
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
    parser = argparse.ArgumentParser(description="Configure local Stripe mock provider instance")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--instance-id")
    parser.add_argument("--secret-key", default="sk_test_local_smoke")
    parser.add_argument("--publishable-key", default="pk_test_local_smoke")
    parser.add_argument("--webhook-secret", default="whsec_local_smoke")
    parser.add_argument("--stripe-api-base", default="http://127.0.0.1:18081")
    args = parser.parse_args()

    token_q = urllib.parse.quote(args.admin_token)
    if args.instance_id:
        instance_id = args.instance_id
    else:
        status, payload = request_json(
            "GET",
            f"{args.api_base}/api/admin/provider-instances?token={token_q}",
        )
        expect(status == 200, f"failed to list provider instances: {status} {payload}")
        stripe_instances = [
            item
            for item in payload.get("instances", [])
            if item.get("providerKey") == "stripe" and item.get("enabled") is True
        ]
        expect(stripe_instances, "no enabled stripe provider instance found")
        instance_id = stripe_instances[0]["id"]

    status, payload = request_json(
        "PUT",
        f"{args.api_base}/api/admin/provider-instances/{urllib.parse.quote(instance_id)}?token={token_q}",
        {
            "config": {
                "secretKey": args.secret_key,
                "publishableKey": args.publishable_key,
                "webhookSecret": args.webhook_secret,
                "apiBase": args.stripe_api_base,
            }
        },
    )
    expect(status == 200, f"failed to update stripe provider instance: {status} {payload}")

    print(
        json.dumps(
            {
                "instanceId": instance_id,
                "apiBase": args.stripe_api_base,
                "webhookSecret": args.webhook_secret,
                "publishableKey": args.publishable_key,
            },
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
