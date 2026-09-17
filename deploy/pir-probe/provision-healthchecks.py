#!/usr/bin/env python3
"""Idempotently provision checks. Run under Infisical prod injection; never prints ping URLs."""

import argparse
import json
import os
import subprocess
import urllib.request

PROJECT = "40862c6d-a089-4355-b405-0477be0ee3b1"
SPECS = [
    ("pir-prod-availability", 60, 120, "PIR_PROBE_HC_AVAILABILITY"),
    ("pir-prod-query", 300, 300, "PIR_PROBE_HC_QUERY"),
    ("pir-prod-watchdog", 300, 300, "WATCHDOG_HEARTBEAT_URL"),
    ("pir-prod-notification-delivery", 60, 120, "PIR_PROBE_HC_DELIVERY"),
]


def api(path, data=None):
    req = urllib.request.Request(
        "https://healthchecks.io/api/v3/" + path,
        headers={
            "X-Api-Key": os.environ["HEALTHCHECKS_API_KEY"],
            "Content-Type": "application/json",
        },
        data=None if data is None else json.dumps(data).encode(),
    )
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.load(r)


def save(name, value):
    r = subprocess.run(
        [
            "infisical",
            "secrets",
            "set",
            name + "=@/dev/stdin",
            "--env=prod",
            "--projectId=" + PROJECT,
            "--path=/",
            "--silent",
        ],
        input=value,
        text=True,
        capture_output=True,
        check=False,
    )
    if r.returncode:
        raise RuntimeError("Infisical write failed for " + name)
    print("Stored " + name + " in vote/prod", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--channel-id")
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()
    channels = api("channels/")["channels"]
    if args.list:
        print(
            json.dumps(
                [
                    {"id": c.get("id"), "name": c.get("name"), "kind": c.get("kind")}
                    for c in channels
                ]
            )
        )
        return
    channel = args.channel_id
    if not channel:
        matches = [
            c
            for c in channels
            if c.get("kind") == "slack" and "thv-alerts" in c.get("name", "")
        ]
        if len(matches) != 1:
            raise RuntimeError(
                "Select the #thv-alerts Slack integration with --channel-id"
            )
        channel = matches[0]["id"]
    if not any(c.get("id") == channel and c.get("kind") == "slack" for c in channels):
        raise RuntimeError("Selected integration is not Slack")
    checks = api("checks/")["checks"]
    for name, period, grace, secret in SPECS:
        matches = [c for c in checks if c["name"] == name]
        if len(matches) > 1:
            raise RuntimeError("Duplicate checks named " + name)
        payload = {
            "name": name,
            "slug": name,
            "timeout": period,
            "grace": grace,
            "tags": "production pir observability",
            "channels": channel,
            "desc": "PIR production monitoring on prod.explorer.valargroup.org; alert and recover in #thv-alerts.",
        }
        path = "checks/" + matches[0]["uuid"] if matches else "checks/"
        check = api(path, payload)
        save(secret, check["ping_url"])
        print(
            json.dumps(
                {"check": name, "period": period, "grace": grace, "id": check["uuid"]}
            ),
            flush=True,
        )


if __name__ == "__main__":
    main()
