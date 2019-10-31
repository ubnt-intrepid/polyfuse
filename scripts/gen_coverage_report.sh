#!/usr/bin/env bash

set -eux

DIR="$(cd $(dirname $BASH_SOURCE)/..; pwd)"
cd $DIR

rm -rfv ./target/debug/deps/polyfuse-*

export CARGO_INCREMENTAL=0
export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Cinline-threshold=0 -Clink-dead-code -Coverflow-checks=off -Zno-landing-pads"

cargo +nightly build --all
cargo +nightly test --all

mkdir -pv target/cov
rm -rfv target/cov/*

grcov ./target/debug/deps -s ./ -o ./target/cov/out.lcov \
  -t lcov \
  --llvm \
  --branch \
  --ignore-not-existing \
  --ignore '/*' \
  --ignore 'target/*'

genhtml -o ./target/cov ./target/cov/out.lcov
