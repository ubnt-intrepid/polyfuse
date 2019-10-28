#![allow(bad_style, clippy::all)]

use polyfuse_sys::abi::*;

include!(concat!(env!("OUT_DIR"), "/abi.rs"));
