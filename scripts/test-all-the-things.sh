#!/usr/bin/env bash

set -e

./scripts/incur fmt
cargo clippy --workspace
cargo build --workspace
cargo build --release --workspace

# for account_simulator
pushd components/ledger_standalone
cargo build
popd

cargo test --no-run
cargo test --release --no-run
cargo test
cargo test --release -j1 -- --ignored --test-threads=1
