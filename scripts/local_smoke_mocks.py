#!/usr/bin/env python3
"""Local mock stack for OPay smoke testing.

Starts two lightweight HTTP servers:
- Platform mock on 127.0.0.1:18080
- Payment provider mock on 127.0.0.1:18081

Only uses Python stdlib so it can run anywhere we have Python 3.
"""

from __future__ import annotations

import argparse
import json
import threading
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlparse


def iso_now(offset_days: int = 0) -> str:
    return (datetime.now(timezone.utc) + timedelta(days=offset_days)).isoformat()


@dataclass
class MockState:
    user_id: int = 42
    username: str = "test-user"
    email: str = "user@example.com"
    notes: str = "Local smoke user"
    balance: float = 88.0
    group_id: int = 101
    channel_group_id: int = 201
    subscription_id: int = 501
    subscription_status: str = "active"
    subscription_expires_at: str = field(default_factory=lambda: iso_now(30))
    fail_next_balance_redeem: int = 0
    fail_next_subscription_redeem: int = 0
    fail_next_balance_add: int = 0
    fail_next_stripe_refund: int = 0
    fail_next_easypay_refund: int = 0
    lock: threading.Lock = field(default_factory=threading.Lock)

    def platform_user(self) -> dict[str, Any]:
        return {
            "id": self.user_id,
            "status": "active",
            "role": "user",
            "email": self.email,
            "username": self.username,
            "notes": self.notes,
            "balance": round(self.balance, 2),
        }

    def platform_group(self) -> dict[str, Any]:
        return {
            "id": self.group_id,
            "name": "Pro Group",
            "status": "active",
            "subscription_type": "subscription",
            "description": "Local smoke subscription group",
            "platform": "openai",
            "rate_multiplier": 0.15,
            "daily_limit_usd": 50,
            "weekly_limit_usd": 200,
            "monthly_limit_usd": 600,
            "default_validity_days": 30,
            "sort_order": 1,
            "supported_model_scopes": ["gpt-4.1-mini", "gpt-4o-mini"],
            "allow_messages_dispatch": True,
            "default_mapped_model": "gpt-4.1-mini",
        }

    def channel_group(self) -> dict[str, Any]:
        return {
            "id": self.channel_group_id,
            "name": "OpenAI Balance Group",
            "status": "active",
            "subscription_type": "balance",
            "description": "Local smoke top-up group",
            "platform": "openai",
            "rate_multiplier": 0.15,
            "daily_limit_usd": 50,
            "weekly_limit_usd": 200,
            "monthly_limit_usd": 600,
            "default_validity_days": None,
            "sort_order": 2,
            "supported_model_scopes": ["gpt-4.1-mini", "gpt-4o-mini"],
            "allow_messages_dispatch": True,
            "default_mapped_model": "gpt-4.1-mini",
        }

    def platform_subscription(self) -> dict[str, Any]:
        return {
            "id": self.subscription_id,
            "user_id": self.user_id,
            "group_id": self.group_id,
            "starts_at": iso_now(-2),
            "status": self.subscription_status,
            "expires_at": self.subscription_expires_at,
            "daily_usage_usd": 0.0,
            "weekly_usage_usd": 0.0,
            "monthly_usage_usd": 0.0,
            "daily_window_start": iso_now(),
            "weekly_window_start": iso_now(),
            "monthly_window_start": iso_now(),
            "assigned_by": 1,
            "assigned_at": iso_now(-2),
            "notes": "Local smoke subscription",
            "created_at": iso_now(-2),
            "updated_at": iso_now(),
        }


def json_response(handler: BaseHTTPRequestHandler, payload: Any, status: int = 200) -> None:
    body = json.dumps(payload).encode("utf-8")
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json; charset=utf-8")
    handler.send_header("Content-Length", str(len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def text_response(handler: BaseHTTPRequestHandler, body: str, status: int = 200) -> None:
    encoded = body.encode("utf-8")
    handler.send_response(status)
    handler.send_header("Content-Type", "text/plain; charset=utf-8")
    handler.send_header("Content-Length", str(len(encoded)))
    handler.end_headers()
    handler.wfile.write(encoded)


def read_json(handler: BaseHTTPRequestHandler) -> dict[str, Any]:
    length = int(handler.headers.get("Content-Length", "0") or "0")
    raw = handler.rfile.read(length) if length > 0 else b"{}"
    if not raw:
        return {}
    return json.loads(raw.decode("utf-8"))


class PlatformHandler(BaseHTTPRequestHandler):
    server_version = "OPayPlatformMock/0.1"

    @property
    def state(self) -> MockState:
        return self.server.state  # type: ignore[attr-defined]

    def log_message(self, format: str, *args: Any) -> None:
        print(f"[platform] {self.address_string()} - {format % args}")

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        path = parsed.path
        query = parse_qs(parsed.query)

        if path == "/api/v1/auth/me":
            return json_response(self, {"data": self.state.platform_user()})

        if path == f"/api/v1/admin/users/{self.state.user_id}":
            return json_response(self, {"data": self.state.platform_user()})

        if path == f"/api/v1/admin/groups/{self.state.group_id}":
            return json_response(self, {"data": self.state.platform_group()})

        if path == f"/api/v1/admin/groups/{self.state.channel_group_id}":
            return json_response(self, {"data": self.state.channel_group()})

        if path == "/api/v1/admin/groups/all":
            return json_response(
                self,
                {"data": [self.state.platform_group(), self.state.channel_group()]},
            )

        if path == f"/api/v1/admin/users/{self.state.user_id}/subscriptions":
            return json_response(self, {"data": [self.state.platform_subscription()]})

        if path == "/api/v1/admin/subscriptions":
            items = [self.state.platform_subscription()]
            return json_response(
                self,
                {
                    "data": {
                        "items": items,
                        "total": len(items),
                        "page": int((query.get("page") or ["1"])[0]),
                        "page_size": int((query.get("page_size") or ["20"])[0]),
                    }
                },
            )

        if path == "/api/v1/admin/users":
            search = (query.get("search") or [""])[0].strip().lower()
            items = []
            if not search or search in self.state.username.lower() or search in self.state.email.lower():
                items.append(
                    {
                        "id": self.state.user_id,
                        "email": self.state.email,
                        "username": self.state.username,
                        "notes": self.state.notes,
                    }
                )
            return json_response(self, {"data": {"items": items}})

        return json_response(self, {"error": f"Unhandled GET {path}"}, status=404)

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        path = parsed.path
        body = read_json(self)

        if path == "/__control/failures":
            with self.state.lock:
                if "fail_next_balance_redeem" in body:
                    self.state.fail_next_balance_redeem = int(body["fail_next_balance_redeem"] or 0)
                if "fail_next_subscription_redeem" in body:
                    self.state.fail_next_subscription_redeem = int(body["fail_next_subscription_redeem"] or 0)
                if "fail_next_balance_add" in body:
                    self.state.fail_next_balance_add = int(body["fail_next_balance_add"] or 0)
                if "fail_next_stripe_refund" in body:
                    self.state.fail_next_stripe_refund = int(body["fail_next_stripe_refund"] or 0)
                if "fail_next_easypay_refund" in body:
                    self.state.fail_next_easypay_refund = int(body["fail_next_easypay_refund"] or 0)
            return json_response(
                self,
                {
                    "success": True,
                    "state": {
                        "fail_next_balance_redeem": self.state.fail_next_balance_redeem,
                        "fail_next_subscription_redeem": self.state.fail_next_subscription_redeem,
                        "fail_next_balance_add": self.state.fail_next_balance_add,
                        "fail_next_stripe_refund": self.state.fail_next_stripe_refund,
                        "fail_next_easypay_refund": self.state.fail_next_easypay_refund,
                    },
                },
            )

        if path == "/api/v1/admin/redeem-codes/create-and-redeem":
            value = float(body.get("value") or 0)
            redeem_type = body.get("type")
            with self.state.lock:
                if redeem_type == "balance" and self.state.fail_next_balance_redeem > 0:
                    self.state.fail_next_balance_redeem -= 1
                    return json_response(
                        self,
                        {"error": "mock balance redeem failure"},
                        status=500,
                    )
                if redeem_type == "subscription" and self.state.fail_next_subscription_redeem > 0:
                    self.state.fail_next_subscription_redeem -= 1
                    return json_response(
                        self,
                        {"error": "mock subscription redeem failure"},
                        status=500,
                    )
                if redeem_type == "balance":
                    self.state.balance += value
                elif redeem_type == "subscription":
                    self.state.subscription_status = "active"
                    self.state.subscription_expires_at = iso_now(int(body.get("validity_days") or 30))
            return json_response(
                self,
                {
                    "redeem_code": {
                        "id": 9001,
                        "code": body.get("code", f"code-{uuid.uuid4().hex[:8]}"),
                        "type": redeem_type,
                        "value": value,
                        "status": "redeemed",
                        "used_by": body.get("user_id"),
                    }
                },
            )

        if path == f"/api/v1/admin/users/{self.state.user_id}/balance":
            amount = float(body.get("balance") or 0)
            operation = body.get("operation")
            with self.state.lock:
                if operation == "add" and self.state.fail_next_balance_add > 0:
                    self.state.fail_next_balance_add -= 1
                    return json_response(
                        self,
                        {"error": "mock balance add failure"},
                        status=500,
                    )
                if operation == "add":
                    self.state.balance += amount
                elif operation == "subtract":
                    self.state.balance -= amount
            return json_response(self, {"success": True, "data": self.state.platform_user()})

        if path == f"/api/v1/admin/subscriptions/{self.state.subscription_id}/extend":
            days = int(body.get("days") or 0)
            with self.state.lock:
                current = datetime.fromisoformat(self.state.subscription_expires_at)
                self.state.subscription_expires_at = (current + timedelta(days=days)).isoformat()
            return json_response(self, {"success": True, "data": self.state.platform_subscription()})

        return json_response(self, {"error": f"Unhandled POST {path}", "body": body}, status=404)


class PaymentProviderHandler(BaseHTTPRequestHandler):
    server_version = "OPayPaymentProviderMock/0.1"

    @property
    def state(self) -> MockState:
        return self.server.state  # type: ignore[attr-defined]

    def log_message(self, format: str, *args: Any) -> None:
        print(f"[payments] {self.address_string()} - {format % args}")

    def do_POST(self) -> None:
        parsed = urlparse(self.path)
        path = parsed.path
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length).decode("utf-8") if length > 0 else ""
        form = parse_qs(raw)

        if path == "/v1/payment_intents":
            payment_intent_id = f"pi_local_{uuid.uuid4().hex[:12]}"
            return json_response(
                self,
                {
                    "id": payment_intent_id,
                    "client_secret": f"{payment_intent_id}_secret_local",
                    "status": "requires_payment_method",
                    "captured_form": form,
                },
            )

        if path == "/v1/refunds":
            with self.state.lock:
                if self.state.fail_next_stripe_refund > 0:
                    self.state.fail_next_stripe_refund -= 1
                    return json_response(
                        self,
                        {"error": "mock stripe refund failure", "captured_form": form},
                        status=500,
                    )
            refund_id = f"re_local_{uuid.uuid4().hex[:12]}"
            return json_response(
                self,
                {
                    "id": refund_id,
                    "status": "succeeded",
                    "captured_form": form,
                },
            )

        if path == "/mapi.php":
            out_trade_no = (form.get("out_trade_no") or [f"order-{uuid.uuid4().hex[:8]}"])[0]
            payment_type = (form.get("type") or ["alipay"])[0]
            return json_response(
                self,
                {
                    "code": 1,
                    "msg": "success",
                    "trade_no": f"easy_trade_{uuid.uuid4().hex[:12]}",
                    "payurl": f"https://mock-easypay.local/pay/{payment_type}/{out_trade_no}",
                    "payurl2": f"https://mock-easypay.local/mobile/{payment_type}/{out_trade_no}",
                    "qrcode": f"easypay://mock/{payment_type}/{out_trade_no}",
                    "captured_form": form,
                },
            )

        if path == "/api.php" and parsed.query == "act=refund":
            with self.state.lock:
                if self.state.fail_next_easypay_refund > 0:
                    self.state.fail_next_easypay_refund -= 1
                    return json_response(
                        self,
                        {"code": 0, "msg": "mock easypay refund failure", "captured_form": form},
                        status=500,
                    )
            return json_response(
                self,
                {
                    "code": 1,
                    "msg": "success",
                    "captured_form": form,
                },
            )

        return json_response(self, {"error": f"Unhandled POST {path}", "form": form}, status=404)


def start_server(
    name: str,
    host: str,
    port: int,
    handler_cls: type[BaseHTTPRequestHandler],
    state: MockState | None = None,
) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer((host, port), handler_cls)
    if state is not None:
        server.state = state  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True, name=name)
    thread.start()
    print(f"[{name}] listening on http://{host}:{port}")
    return server


def main() -> int:
    parser = argparse.ArgumentParser(description="Run local OPay smoke mocks")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--platform-port", type=int, default=18080)
    parser.add_argument("--payment-port", type=int, default=18081)
    args = parser.parse_args()

    state = MockState()
    platform_server = start_server("platform", args.host, args.platform_port, PlatformHandler, state)
    payment_server = start_server("payments", args.host, args.payment_port, PaymentProviderHandler, state)

    print("[local-smoke-mocks] ready")
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        print("\n[local-smoke-mocks] shutting down")
    finally:
        platform_server.shutdown()
        payment_server.shutdown()
        platform_server.server_close()
        payment_server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
