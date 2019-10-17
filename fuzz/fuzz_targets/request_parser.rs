#![no_main]

use libfuzzer_sys::fuzz_target;
use polyfuse::abi::parse::Parser;

fuzz_target!(|data: &[u8]| {
    let mut parser = Parser::new(data);
    let _ = parser.parse();
});
