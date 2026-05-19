#!/usr/bin/env python3
"""Trigger N posts to produce N commits on the Aurora-Locus firehose.

For Arc 14 Phase B Scenario 4 (ConsumerTooSlow): fires enough
commits in a short window to overflow the server-side 100-deep
mpsc buffer when the consumer is reading slowly.

# Usage

    phase-b/trigger-commits.py \\
        --did "$DID" \\
        --jwt "$JWT" \\
        --count 150 \\
        [--base-url http://localhost:3000] \\
        [--collection app.bsky.feed.post]

Each iteration POSTs to `com.atproto.repo.createRecord` with a
unique post body (text includes the iteration number so the
records are not deduplicated). Prints progress every 25 records;
prints final commit count + duration.

# Failure behavior

  - HTTP non-2xx: prints status + body and continues (unless
    `--strict`, in which case it aborts).
  - Auth failure (401): aborts immediately — the JWT must be
    valid for the lifetime of the run.

# Dependencies

  - Python 3.8+ (uses urllib from stdlib — no `requests` dep).
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone


def _create_record(
    base_url: str,
    jwt: str,
    did: str,
    collection: str,
    text: str,
) -> tuple[int, str]:
    """POST com.atproto.repo.createRecord. Returns (status, body)."""
    payload = json.dumps(
        {
            "repo": did,
            "collection": collection,
            "record": {
                "$type": collection,
                "text": text,
                "createdAt": datetime.now(timezone.utc)
                .isoformat()
                .replace("+00:00", "Z"),
            },
        }
    ).encode("utf-8")

    req = urllib.request.Request(
        url=f"{base_url}/xrpc/com.atproto.repo.createRecord",
        data=payload,
        headers={
            "content-type": "application/json",
            "authorization": f"Bearer {jwt}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status, resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace") if e.fp else ""
        return e.code, body


def main() -> int:
    p = argparse.ArgumentParser(
        prog="trigger-commits.py",
        description=(
            "Create N posts to produce N firehose commits. For "
            "Arc 14 Phase B Scenario 4 (ConsumerTooSlow) setup."
        ),
    )
    p.add_argument("--did", required=True, help="actor DID")
    p.add_argument("--jwt", required=True, help="accessJwt bearer token")
    p.add_argument(
        "--count", type=int, required=True, help="number of records to create"
    )
    p.add_argument(
        "--base-url",
        default="http://localhost:3000",
        help="PDS base URL (default: http://localhost:3000)",
    )
    p.add_argument(
        "--collection",
        default="app.bsky.feed.post",
        help="record collection NSID (default: app.bsky.feed.post)",
    )
    p.add_argument(
        "--strict",
        action="store_true",
        help="abort on first HTTP non-2xx (default: continue + count failures)",
    )
    args = p.parse_args()

    if args.count <= 0:
        sys.stderr.write("error: --count must be > 0\n")
        return 2

    start = time.monotonic()
    successes = 0
    failures = 0

    for i in range(1, args.count + 1):
        status, body = _create_record(
            args.base_url,
            args.jwt,
            args.did,
            args.collection,
            text=f"phase-b trigger-commits #{i}",
        )
        if 200 <= status < 300:
            successes += 1
        else:
            failures += 1
            sys.stderr.write(f"  [#{i}] HTTP {status}: {body[:200]}\n")
            if status == 401:
                sys.stderr.write(
                    "  ! 401 Unauthorized — JWT is invalid/expired; aborting.\n"
                )
                return 4
            if args.strict:
                sys.stderr.write("  ! --strict mode; aborting on first failure.\n")
                return 5
        if i % 25 == 0 or i == args.count:
            elapsed = time.monotonic() - start
            rate = i / elapsed if elapsed > 0 else 0.0
            print(
                f"[{i}/{args.count}] ok={successes} fail={failures} "
                f"elapsed={elapsed:.1f}s ({rate:.1f}/s)",
                file=sys.stderr,
            )

    total = time.monotonic() - start
    print(
        f"\ndone: {successes}/{args.count} succeeded "
        f"({failures} failed) in {total:.1f}s"
    )
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
