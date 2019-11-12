#!/usr/bin/env bash

set -eux

DIR="$(cd $(dirname $BASH_SOURCE)/..; pwd)"
cd $DIR

rm -rfv ./target/debug/deps/polyfuse*
rm -rfv ./target/debug/deps/example*

export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Cinline-threshold=0 -Clink-dead-code -Coverflow-checks=off -Zno-landing-pads"

cargo +nightly build --workspace --all-targets
cargo +nightly test --workspace

mkdir -pv target/cov
rm -rfv target/cov/*

grcov ./target/debug/deps -s ./ -o ./target/cov \
  -t html \
  --llvm \
  --branch \
  --ignore-not-existing \
  --ignore '/*' \
  --ignore 'target/*'
