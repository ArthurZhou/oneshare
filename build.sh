#!/usr/bin/env bash
#
# OneShare release build — produces a fully static musl binary.
#
# Requires:
#   - Rust toolchain with the musl target installed:
#       rustup target add x86_64-unknown-linux-musl
#   - musl-gcc (package `musl-tools`)
#   - cmake + perl (build deps of aws-lc-sys / ring, pulled in by reqwest+rustls)
#
# Usage:
#   ./build.sh                                  # default: x86_64-unknown-linux-musl
#   TARGET=aarch64-unknown-linux-musl ./build.sh
#
set -euo pipefail

TARGET="${TARGET:-x86_64-unknown-linux-musl}"

# aws-lc-sys / ring must be compiled with a C toolchain that targets musl.
export CC="${CC:-musl-gcc}"
# The final link step must also use the musl toolchain.
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-musl-gcc}"

echo ">> Building oneshare --target $TARGET --release"
cargo build --release --target "$TARGET"

BIN="target/$TARGET/release/oneshare"
if [ ! -x "$BIN" ]; then
  echo "ERROR: binary not found at $BIN" >&2
  exit 1
fi

echo ">> Build OK: $BIN"
file "$BIN"
