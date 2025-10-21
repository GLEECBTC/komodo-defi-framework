#![allow(unused_imports)]
use crate::{log, set_panic_hook, stack_trace, stack_trace_frame, writeln};
use std::env;
use std::io::stderr;
use std::io::Write;
use std::os::raw::c_int;
use std::path::Path;
use std::process::abort;
use std::sync::Once;

#[cfg(windows)]
#[allow(dead_code)]
fn exception_name(exception_code: u32) -> &'static str {
    use winapi::um::minwinbase::{EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION};
    match exception_code {
        EXCEPTION_ACCESS_VIOLATION => "Access Violation",
        EXCEPTION_ILLEGAL_INSTRUCTION => "Illegal Instruction",
        0xE06D7363 => "VC++ Exception", // https://blogs.msdn.microsoft.com/oldnewthing/20100730-00/?p=13273
        _ => "",
    }
}

/// Performs a crash report and aborts.
#[cfg(windows)]
#[no_mangle]
pub extern "C" fn rust_seh_handler(exception_code: u32) {
    writeln("SEH caught!"); // Write something without heap allocation first.
    writeln(&format!(
        "ExceptionCode: {} ({}).",
        exception_code,
        exception_name(exception_code)
    ));
    stack_trace(&mut stack_trace_frame, &mut |trace| {
        if !trace.is_empty() {
            writeln(trace.trim_end())
        }
    });
    abort()
}

#[cfg(test)]
#[inline(never)]
#[allow(dead_code)]
fn access_violation() {
    let ptr: *mut i32 = std::ptr::null_mut::<i32>();
    unsafe { *ptr = 123 };
}

#[cfg(test)]
#[inline(never)]
#[allow(dead_code)]
extern "C" fn call_access_violation() {
    access_violation()
}

#[cfg(unix)]
extern "C" fn signal_handler(sig: c_int) {
    writeln("Signal caught!"); // Write something without heap allocation first.

    let sigˢ;
    let sigⁿ = match sig {
        libc::SIGILL => "SIGILL",
        libc::SIGFPE => "SIGFPE",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGBUS => "SIGBUS",
        libc::SIGSYS => "SIGSYS",
        sig => {
            sigˢ = sig.to_string();
            &sigˢ[..]
        },
    };

    writeln(sigⁿ);
    stack_trace(&mut stack_trace_frame, &mut |trace| {
        if !trace.is_empty() {
            writeln(trace.trim_end())
        }
    });
    abort();
}

#[cfg(unix)]
fn init_signal_handling() {
    use std::mem::{zeroed, MaybeUninit};

    let mut sa: libc::sigaction = unsafe { zeroed() };
    sa.sa_sigaction = signal_handler as *const extern "C" fn(c_int) as usize;
    for &signal in [libc::SIGILL, libc::SIGFPE, libc::SIGSEGV, libc::SIGBUS, libc::SIGSYS].iter() {
        let mut prev = MaybeUninit::<libc::sigaction>::uninit();
        let rc = unsafe { libc::sigaction(signal, &sa, prev.as_mut_ptr()) };
        if rc != 0 {
            log::error!("Error {} invoking sigaction on {}", rc, signal)
        }
    }
}

#[allow(dead_code)]
#[cfg(not(unix))]
fn init_signal_handling() {}

/// Setup the crash handlers.
#[allow(dead_code)]
#[cfg(not(target_arch = "wasm32"))]
pub fn init_crash_reports() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        crate::wio::init();

        set_panic_hook();

        // Try to invoke the `rust_seh_handler` whenever the C code crashes.
        #[cfg(windows)]
        {
            extern "C" {
                fn init_veh();
            }
            unsafe {
                init_veh();
            }
        }

        #[cfg(unix)]
        {
            init_signal_handling();
        }

        // Log Rust panics.
        env::set_var("RUST_BACKTRACE", "1")
        // ^^ NB: In the future this might also affect the normal errors, cf. https://github.com/rust-lang/rfcs/blob/master/text/2504-fix-error.md.
    })
}

#[cfg(target_arch = "wasm32")]
pub fn init_crash_reports() {
    unimplemented!()
}

// Make sure Rust panics still work in the presence of the VEH handler.
#[test]
#[should_panic]
fn test_panic() {
    init_crash_reports();
    panic!("NP");
}
