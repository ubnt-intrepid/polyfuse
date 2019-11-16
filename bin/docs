#!/bin/bash

DIR="$(cd $(dirname $BASH_SOURCE)/..; pwd)"
echo "DIR=${DIR}"

set -ex

cargo fetch
rm -rfv $DIR/target/doc

timeout -sKILL 900 cargo doc --no-deps -p polyfuse-sys
timeout -sKILL 900 cargo doc --no-deps -p polyfuse
timeout -sKILL 900 cargo doc --no-deps -p polyfuse-tokio

rm -rfv $DIR/target/doc/.lock

echo '<meta http-equiv="refresh" content="0;url=polyfuse">' > $DIR/target/doc/index.html
