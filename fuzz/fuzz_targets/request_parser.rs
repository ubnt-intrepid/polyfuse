#![no_main]

use fuse_async::request::Parser;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut parser = Parser::new(data);
    let _ = parser.parse();
});
