#!/usr/bin/env python3
import argparse
import json
import sys
import urllib.request

DEFAULT_MCP_URL = "http://127.0.0.1:44129/mcp"


def rpc(url: str, method: str, params: dict, req_id: str) -> dict:
    payload = {
        "jsonrpc": "2.0",
        "id": req_id,
        "method": method,
        "params": params,
    }
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, headers={"content-type": "application/json"}, method="POST"
    )
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read().decode("utf-8"))


def tools_list(url: str) -> int:
    out = rpc(url, "tools/list", {}, "tools-list")
    print(json.dumps(out, ensure_ascii=False, indent=2))
    return 0


def tools_call(url: str, name: str, arguments: dict) -> int:
    out = rpc(
        url,
        "tools/call",
        {"name": name, "arguments": arguments},
        "tools-call",
    )
    print(json.dumps(out, ensure_ascii=False, indent=2))
    return 0


def apply_plan_v2(url: str, payload_path: str) -> int:
    with open(payload_path, "r", encoding="utf-8") as f:
        args = json.load(f)
    return tools_call(url, "axon_soll_apply_plan_v2", args)


def main() -> int:
    parser = argparse.ArgumentParser(description="Axon SOLL MCP helper (HTTP JSON-RPC)")
    parser.add_argument("--url", default=DEFAULT_MCP_URL, help="MCP endpoint URL")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("list-tools")

    p_call = sub.add_parser("call")
    p_call.add_argument("--tool", required=True)
    p_call.add_argument("--args-json", default="{}")

    p_apply = sub.add_parser("apply-plan-v2")
    p_apply.add_argument("--payload", required=True, help="Path to JSON payload file")

    args = parser.parse_args()

    if args.cmd == "list-tools":
        return tools_list(args.url)
    if args.cmd == "call":
        tool_args = json.loads(args.args_json)
        return tools_call(args.url, args.tool, tool_args)
    if args.cmd == "apply-plan-v2":
        return apply_plan_v2(args.url, args.payload)
    return 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise
