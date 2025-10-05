#![allow(bad_style, deprecated, clippy::all)]

use polyfuse_kernel::*;

include!(concat!(env!("OUT_DIR"), "/kernel.rs"));
