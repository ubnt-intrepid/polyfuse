#!/usr/bin/env bash

set -eux

rm -rfv target/debug/deps/fuse_async-*
rm -rfv target/debug/deps/fuse_async_channel-*
rm -rfv target/debug/deps/example_null-*

export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Cinline-threshold=0 -Clink-dead-code -Coverflow-checks=off -Zno-landing-pads"

cargo +nightly build --all --verbose
cargo +nightly test --all --verbose

mkdir -pv ./cov
rm -rf ./cov/*

grcov ./target/debug/deps -s . -t html --llvm --branch --ignore-not-existing -o ./cov --ignore '/*'
