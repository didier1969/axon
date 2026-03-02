"""Entry point for daemon subprocess: python -m axon.daemon [--max-dbs N]"""
from __future__ import annotations

import argparse
import asyncio
import logging


def main() -> None:
    parser = argparse.ArgumentParser(description="Axon daemon server")
    parser.add_argument("--max-dbs", type=int, default=5, help="Max cached KuzuDB backends")
    parser.add_argument("--log-level", default="WARNING", help="Log level")
    args = parser.parse_args()

    logging.basicConfig(level=args.log_level)
    from axon.daemon.server import run_daemon
    asyncio.run(run_daemon(args.max_dbs))


if __name__ == "__main__":
    main()
