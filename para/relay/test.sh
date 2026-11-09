#!/usr/bin/env bash
# Rebuild the exact reviewed sources and run both unit and real VLPDS wire tests.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
mkdir -p "$ROOT/target"
BUILD_DIR="$(mktemp -d "$ROOT/target/para-rsky-test.XXXXXX")"
"$HERE/prepare.sh" "$BUILD_DIR/source"
export RUSTUP_TOOLCHAIN=1.99.0
export CARGO_TARGET_DIR="$ROOT/target/rsky-relay-checks"
cargo test --locked --manifest-path "$BUILD_DIR/source/Cargo.toml" -p rsky-relay --no-default-features
cargo build --locked --manifest-path "$BUILD_DIR/source/Cargo.toml" -p rsky-relay --bin rsky-relay
RELAY_BIN="$CARGO_TARGET_DIR/debug/rsky-relay"
export RELAY_BIN
unset CARGO_TARGET_DIR
cargo build --locked --manifest-path "$ROOT/Cargo.toml" --bin vlpds
node "$HERE/tests/smoke.mjs"
