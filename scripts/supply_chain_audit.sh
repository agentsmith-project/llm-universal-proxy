#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo metadata --locked --format-version 1 --no-deps >/dev/null
cargo audit --ignore RUSTSEC-2026-0185 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2026-0190
