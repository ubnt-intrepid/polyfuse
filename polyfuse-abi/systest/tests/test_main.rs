#![allow(bad_style, unused_imports)]

use libc::*;
use polyfuse_abi::*;

include!(concat!(env!("OUT_DIR"), "/all.rs"));
