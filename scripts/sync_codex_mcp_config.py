#!/usr/bin/env python3
import argparse
from pathlib import Path


def upsert_server_block(text: str, section_name: str, url: str) -> str:
    lines = text.splitlines()
    header = f"[mcp_servers.{section_name}]"
    i = 0
    found = False
    while i < len(lines):
        if lines[i].strip() == header:
            found = True
            j = i + 1
            while j < len(lines) and not (
                lines[j].startswith("[") and lines[j].endswith("]")
            ):
                j += 1

            block = lines[i:j]
            replaced = False
            for idx in range(1, len(block)):
                if block[idx].lstrip().startswith("url ="):
                    block[idx] = f'url = "{url}"'
                    replaced = True
                    break
            if not replaced:
                block.insert(1, f'url = "{url}"')
            lines[i:j] = block
            break
        i += 1

    if not found:
        if lines and lines[-1].strip():
            lines.append("")
        lines.extend([header, f'url = "{url}"'])

    rendered = "\n".join(lines)
    if not rendered.endswith("\n"):
        rendered += "\n"
    return rendered


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("--live-url", required=True)
    parser.add_argument("--dev-url", required=True)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()

    config_path = Path(args.config).expanduser()
    original = config_path.read_text() if config_path.exists() else ""
    updated = upsert_server_block(original, "axon-live", args.live_url)
    updated = upsert_server_block(updated, "axon-dev", args.dev_url)

    if not args.apply:
        print(updated)
        return 0

    config_path.write_text(updated)
    print(f"Updated {config_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
