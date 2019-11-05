#![allow(bad_style, clippy::all)]

use polyfuse_sys::kernel::*;

include!(concat!(env!("OUT_DIR"), "/kernel.rs"));
