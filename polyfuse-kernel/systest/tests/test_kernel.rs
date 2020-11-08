#![allow(bad_style, clippy::all)]

use polyfuse_kernel::*;

include!(concat!(env!("OUT_DIR"), "/kernel.rs"));
