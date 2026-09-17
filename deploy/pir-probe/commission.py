#!/usr/bin/env python3
"""Run bounded commissioning cycles without emitting alerts or heartbeats."""

import argparse
import importlib.machinery
import importlib.util
import json
import threading
from pathlib import Path

loader = importlib.machinery.SourceFileLoader(
    "probe", str(Path(__file__).with_name("pir-probe"))
)
spec = importlib.util.spec_from_loader(loader.name, loader)
p = importlib.util.module_from_spec(spec)
loader.exec_module(p)
a = argparse.ArgumentParser()
a.add_argument("--config", required=True)
a.add_argument("--state-dir", required=True)
a.add_argument("--cycles", type=int, default=3)
args = a.parse_args()
c = p.validate_config(json.loads(Path(args.config).read_text()))
s = p.State(Path(args.state_dir) / "status.json", notify=False)
m = p.Monitor(c, s, str(Path(__file__).with_name("pir-probe-query")), threading.Event())
m.hb = {k: "" for k in m.hb}
for i in range(args.cycles):
    m.availability_cycle()
    m.query_cycle()
    bad = [key for key, value in s.data["checks"].items() if not value["result"]["ok"]]
    print(json.dumps({"commission_cycle": i + 1, "failed": bad}), flush=True)
    if bad:
        raise SystemExit(1)
