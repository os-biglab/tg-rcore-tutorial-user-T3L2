#![no_std]
//!
//! 教程阅读建议：
//!
//! - `_start` 展示用户程序最小运行时（初始化控制台、堆、调用 main）；
//! - 其余辅助函数（sleep/pipe_*）展示了常见 syscall 组合用法。

mod heap;
mod tangram;

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering};

pub use tg_console::{print, println};
pub use tg_syscall::*;

const SYSCALL_RENDER_BLOCK: usize = 0x1000_0001;
const FRAMEBUFFER_WIDTH: usize = 1280;
const FRAMEBUFFER_HEIGHT: usize = 800;
const FRAMEBUFFER_BYTES: usize = FRAMEBUFFER_WIDTH * FRAMEBUFFER_HEIGHT * 4;
const FRAMEBUFFER_TRANSPARENT: u32 = 0x0000_0000;

#[repr(align(16))]
struct RenderBuffer([u8; FRAMEBUFFER_BYTES]);

#[unsafe(link_section = ".bss.uninit")]
static mut RENDER_BUFFER: RenderBuffer = RenderBuffer([0; FRAMEBUFFER_BYTES]);

pub const BLOCK_COUNT: usize = tangram::BLOCK_COUNT;

static USER_RUNTIME_INIT: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "riscv64")]
fn submit_framebuffer(framebuffer: &[u8]) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") framebuffer.as_ptr() as isize => ret,
            in("a1") framebuffer.len(),
            in("a2") FRAMEBUFFER_WIDTH,
            in("a3") FRAMEBUFFER_HEIGHT,
            in("a7") SYSCALL_RENDER_BLOCK,
        );
    }
    ret
}

#[cfg(not(target_arch = "riscv64"))]
fn submit_framebuffer(_framebuffer: &[u8]) -> isize {
    -1
}

pub fn render_block(block: usize) -> isize {
    let framebuffer = unsafe {
        let ptr = core::ptr::addr_of_mut!(RENDER_BUFFER.0) as *mut u8;
        core::slice::from_raw_parts_mut(ptr, FRAMEBUFFER_BYTES)
    };
    // 这里不用clear也行，反正和上次是增量的
    // clear_framebuffer(framebuffer, FRAMEBUFFER_TRANSPARENT);
    tangram::render_block_by_index(framebuffer, FRAMEBUFFER_WIDTH, FRAMEBUFFER_HEIGHT, block);
    submit_framebuffer(framebuffer)
}

// fn clear_framebuffer(framebuffer: &mut [u8], color: u32) {
//     let pixel = color.to_le_bytes();
//     for chunk in framebuffer.chunks_exact_mut(4) {
//         chunk.copy_from_slice(&pixel);
//     }
// }

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // 用户态运行时初始化只执行一次：避免批处理复用地址时重复初始化导致 panic。
    if !USER_RUNTIME_INIT.swap(true, Ordering::AcqRel) {
        tg_console::init_console(&Console);
    }

    unsafe extern "C" {
        fn main() -> i32;
    }

    // SAFETY: main 函数由用户程序提供，链接器保证其存在且符合 C ABI
    exit(unsafe { main() });
    unreachable!()
}

#[panic_handler]
fn panic_handler(panic_info: &core::panic::PanicInfo) -> ! {
    let err = panic_info.message();
    if let Some(location) = panic_info.location() {
        println!("[user-panic] {}:{} {err}", location.file(), location.line());
    } else {
        println!("[user-panic] {err}");
    }
    exit(1);
    unreachable!()
}

pub fn getchar() -> u8 {
    let mut c = [0u8; 1];
    read(STDIN, &mut c);
    c[0]
}

struct Console;

impl tg_console::Console for Console {
    #[inline]
    fn put_char(&self, c: u8) {
        tg_syscall::write(STDOUT, &[c]);
    }

    #[inline]
    fn put_str(&self, s: &str) {
        tg_syscall::write(STDOUT, s.as_bytes());
    }
}

pub fn sleep(period_ms: usize) {
    // 轮询时钟 + 主动让出 CPU 的教学实现，便于理解 time/yield 系统调用协作。
    let mut time: TimeSpec = TimeSpec::ZERO;
    clock_gettime(ClockId::CLOCK_MONOTONIC, &mut time as *mut _ as _);
    let time = time + TimeSpec::from_millsecond(period_ms);
    loop {
        let mut now: TimeSpec = TimeSpec::ZERO;
        clock_gettime(ClockId::CLOCK_MONOTONIC, &mut now as *mut _ as _);
        if now > time {
            break;
        }
        sched_yield();
    }
}

pub fn get_time() -> isize {
    let mut time: TimeSpec = TimeSpec::ZERO;
    clock_gettime(ClockId::CLOCK_MONOTONIC, &mut time as *mut _ as _);
    (time.tv_sec * 1000 + time.tv_nsec / 1_000_000) as isize
}

pub fn trace_read(ptr: *const u8) -> Option<u8> {
    let ret = trace(0, ptr as usize, 0);
    if ret >= 0 && ret <= 255 {
        Some(ret as u8)
    } else {
        None
    }
}

pub fn trace_write(ptr: *const u8, value: u8) -> isize {
    trace(1, ptr as usize, value as usize)
}

pub fn count_syscall(syscall_id: usize) -> isize {
    trace(2, syscall_id, 0)
}

/// 从管道读取数据
/// 返回实际读取的总字节数，负数表示错误
pub fn pipe_read(pipe_fd: usize, buffer: &mut [u8]) -> isize {
    let mut total_read = 0usize;
    let len = buffer.len();
    loop {
        if total_read >= len {
            return total_read as isize;
        }
        let ret = read(pipe_fd, &mut buffer[total_read..]);
        if ret == -2 {
            // 暂时无数据，让出 CPU 后重试
            sched_yield();
            continue;
        } else if ret == 0 {
            // EOF，写端关闭
            return total_read as isize;
        } else if ret < 0 {
            // 其他错误
            return ret;
        } else {
            total_read += ret as usize;
        }
    }
}

/// 向管道写入数据
/// 返回实际写入的总字节数，负数表示错误
pub fn pipe_write(pipe_fd: usize, buffer: &[u8]) -> isize {
    let mut total_write = 0usize;
    let len = buffer.len();
    loop {
        if total_write >= len {
            return total_write as isize;
        }
        let ret = write(pipe_fd, &buffer[total_write..]);
        if ret == -2 {
            // 缓冲区满，让出 CPU 后重试
            sched_yield();
            continue;
        } else if ret < 0 {
            // 其他错误
            return ret;
        } else {
            total_write += ret as usize;
        }
    }
}
