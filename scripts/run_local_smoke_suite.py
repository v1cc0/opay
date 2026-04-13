#!/usr/bin/env python3
"""Run the local API smoke suite end-to-end with automatic process orchestration."""

from __future__ import annotations

import argparse
import json
import os
import signal
import shlex
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


def wait_for_json(url: str, timeout_seconds: int = 30) -> dict[str, Any]:
    deadline = time.time() + timeout_seconds
    last_error: str | None = None
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=5) as resp:
                raw = resp.read().decode("utf-8")
                return json.loads(raw) if raw else {}
        except Exception as exc:  # noqa: BLE001
            last_error = str(exc)
            time.sleep(0.5)
    raise RuntimeError(f"timed out waiting for {url}: {last_error}")


class ManagedProcess:
    def __init__(self, name: str, cmd: list[str], cwd: Path, log_path: Path):
        self.name = name
        self.cmd = cmd
        self.cwd = cwd
        self.log_path = log_path
        self.log_file = None
        self.proc: subprocess.Popen[str] | None = None

    def start(self) -> None:
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        self.log_file = self.log_path.open("w")
        self.proc = subprocess.Popen(
            self.cmd,
            cwd=self.cwd,
            stdout=self.log_file,
            stderr=subprocess.STDOUT,
            text=True,
        )

    def stop(self) -> None:
        if self.proc is None:
            return
        if self.proc.poll() is None:
            self.proc.send_signal(signal.SIGINT)
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        if self.log_file is not None:
            self.log_file.close()
            self.log_file = None
        self.proc = None


def run_capture(cmd: list[str], cwd: Path) -> str:
    result = subprocess.run(cmd, cwd=cwd, text=True, capture_output=True, check=True)
    return result.stdout


def run_print(cmd: list[str], cwd: Path) -> None:
    subprocess.run(cmd, cwd=cwd, check=True)


def run_check(cmd: list[str], cwd: Path, env: dict[str, str] | None = None) -> None:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    subprocess.run(cmd, cwd=cwd, check=True, env=merged_env)


def run_check_to_log(
    cmd: list[str],
    cwd: Path,
    log_path: Path,
    env: dict[str, str] | None = None,
) -> None:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("w") as log_file:
        subprocess.run(
            cmd,
            cwd=cwd,
            check=True,
            env=merged_env,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            text=True,
        )


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.write_text(json.dumps(payload, indent=2))


def main() -> int:
    parser = argparse.ArgumentParser(description="Run local OPay smoke suite")
    parser.add_argument("--repo-root", default=".")
    parser.add_argument("--api-base", default="http://127.0.0.1:8080")
    parser.add_argument("--health-url", default="http://127.0.0.1:8080/healthz")
    parser.add_argument("--platform-port", type=int, default=18080)
    parser.add_argument("--stripe-port", type=int, default=18081)
    parser.add_argument("--admin-token", default="opay-admin-smoke-token")
    parser.add_argument("--user-id", type=int, default=42)
    parser.add_argument("--output", default="/tmp/opay-local-smoke-suite.json")
    parser.add_argument("--logs-dir", default="/tmp/opay-local-smoke-logs")
    parser.add_argument("--with-browser", action="store_true")
    parser.add_argument("--browser-base-url", default="http://127.0.0.1:8787")
    parser.add_argument("--browser-runner-cmd", default=os.environ.get("OPAY_BROWSER_RUNNER_CMD", ""))
    args = parser.parse_args()

    repo_root = Path(args.repo_root).resolve()
    logs_dir = Path(args.logs_dir)
    output_path = Path(args.output)
    logs_dir.mkdir(parents=True, exist_ok=True)

    mocks = ManagedProcess(
        "local-mocks",
        [sys.executable, "scripts/local_smoke_mocks.py"],
        repo_root,
        logs_dir / "local_smoke_mocks.log",
    )
    backend = ManagedProcess(
        "backend",
        ["cargo", "run"],
        repo_root,
        logs_dir / "backend.log",
    )
    frontend = ManagedProcess(
        "frontend",
        ["pnpm", "--dir", "frontend", "cf:dev", "--", "--port", "8787"],
        repo_root,
        logs_dir / "frontend.log",
    )

    summary: dict[str, Any] = {"steps": []}
    try:
        mocks.start()
        time.sleep(1)

        backend.start()
        health = wait_for_json(args.health_url, timeout_seconds=45)
        summary["health"] = health

        browser_enabled = bool(args.with_browser)
        browser_runner_cmd = args.browser_runner_cmd.strip()
        if browser_enabled:
            if not browser_runner_cmd:
                raise RuntimeError(
                    "--with-browser requires --browser-runner-cmd or OPAY_BROWSER_RUNNER_CMD"
                )
            frontend.start()
            wait_for_json(f"{args.browser_base_url}/healthz", timeout_seconds=60)

        stripe_config_output = run_capture(
            [
                sys.executable,
                "scripts/configure_local_stripe_mock.py",
                "--api-base",
                args.api_base,
                "--admin-token",
                args.admin_token,
                "--stripe-api-base",
                f"http://127.0.0.1:{args.stripe_port}",
            ],
            repo_root,
        )
        summary["stripeMockConfig"] = json.loads(stripe_config_output)
        summary["steps"].append("configured_stripe_mock")

        easypay_config_output = run_capture(
            [
                sys.executable,
                "scripts/configure_local_easypay_mock.py",
                "--api-base",
                args.api_base,
                "--admin-token",
                args.admin_token,
                "--payment-api-base",
                f"http://127.0.0.1:{args.stripe_port}",
            ],
            repo_root,
        )
        summary["easyPayMockConfig"] = json.loads(easypay_config_output)
        summary["steps"].append("configured_easypay_mock")

        webhook_output = run_capture(
            [sys.executable, "scripts/stripe_webhook_completion_smoke.py"],
            repo_root,
        )
        summary["stripeWebhookCompletion"] = json.loads(webhook_output)
        summary["steps"].append("stripe_webhook_completion")

        stripe_recovery_output = run_capture(
            [sys.executable, "scripts/stripe_recovery_smoke.py"],
            repo_root,
        )
        summary["stripeRecovery"] = json.loads(stripe_recovery_output)
        summary["steps"].append("stripe_recovery")

        stripe_refund_recovery_output = run_capture(
            [sys.executable, "scripts/stripe_refund_recovery_smoke.py"],
            repo_root,
        )
        summary["stripeRefundRecovery"] = json.loads(stripe_refund_recovery_output)
        summary["steps"].append("stripe_refund_recovery")

        stripe_refund_manual_recovery_output = run_capture(
            [sys.executable, "scripts/stripe_refund_manual_recovery_smoke.py"],
            repo_root,
        )
        summary["stripeRefundManualRecovery"] = json.loads(stripe_refund_manual_recovery_output)
        summary["steps"].append("stripe_refund_manual_recovery")

        easy_pay_output = run_capture(
            [sys.executable, "scripts/easypay_notify_completion_smoke.py"],
            repo_root,
        )
        summary["easyPayNotifyCompletion"] = json.loads(easy_pay_output)
        summary["steps"].append("easypay_notify_completion")

        backend.stop()
        seed_output = run_capture(
            [
                "cargo",
                "run",
                "--quiet",
                "--example",
                "seed_admin_orders",
                "--",
                "--db-path",
                "data/opay-smoke.db",
                "--user-id",
                str(args.user_id),
            ],
            repo_root,
        )
        seed_payload = json.loads(seed_output)
        ids_path = logs_dir / "admin_order_actions_ids.json"
        write_json(ids_path, seed_payload)
        summary["adminSeed"] = seed_payload
        summary["steps"].append("seeded_admin_orders")

        backend.start()
        wait_for_json(args.health_url, timeout_seconds=45)

        admin_output = run_capture(
            [
                sys.executable,
                "scripts/admin_order_actions_smoke.py",
                "--skip-seed",
                "--ids-file",
                str(ids_path),
            ],
            repo_root,
        )
        summary["adminOrderActions"] = json.loads(admin_output)
        summary["steps"].append("admin_order_actions")

        concurrent_output = run_capture(
            ["cargo", "run", "--quiet", "--example", "concurrent_write_smoke"],
            repo_root,
        )
        summary["concurrentWriteSmoke"] = json.loads(concurrent_output)
        summary["steps"].append("concurrent_write_smoke")

        write_json(output_path, summary)

        if browser_enabled:
            browser_result_path = logs_dir / "browser_smoke_result.json"
            browser_output_dir = logs_dir / "browser"
            browser_output_dir.mkdir(parents=True, exist_ok=True)
            run_check_to_log(
                shlex.split(browser_runner_cmd) + [str(repo_root / "scripts" / "browser_smoke_suite.js")],
                repo_root,
                logs_dir / "browser_smoke.log",
                env={
                    "OPAY_SMOKE_SUMMARY_PATH": str(output_path),
                    "OPAY_BROWSER_RESULT_PATH": str(browser_result_path),
                    "OPAY_BROWSER_OUTPUT_DIR": str(browser_output_dir),
                    "OPAY_BROWSER_BASE_URL": args.browser_base_url,
                    "OPAY_BROWSER_HEADLESS": "1",
                },
            )
            summary["browserSmoke"] = json.loads(browser_result_path.read_text())
            summary["steps"].append("browser_smoke")

        write_json(output_path, summary)
        print(json.dumps(summary, indent=2))
        return 0
    finally:
        frontend.stop()
        backend.stop()
        mocks.stop()


if __name__ == "__main__":
    raise SystemExit(main())
