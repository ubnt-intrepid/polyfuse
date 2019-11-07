#![allow(bad_style, clippy::all, unused_imports, unused_macros)]

use polyfuse_sys::libfuse::*;

include!(concat!(env!("OUT_DIR"), "/libfuse.rs"));
