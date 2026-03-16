#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    let _ = user_lib::render_block(10);
    println!("[t3l2] render step 10");
    0
}
