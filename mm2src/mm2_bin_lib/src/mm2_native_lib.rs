use super::*;
use common::crash_reports::init_crash_reports;
use common::executor::SpawnFuture;
use common::log::{register_callback, FfiCallback};
use common::{block_on, log, now_float};
use enum_primitive_derive::Primitive;
use gstuff::any_to_str;
use libc::c_char;
use mm2_core::mm_ctx::MmArc;
use mm2_main::LpMainParams;
use num_traits::FromPrimitive;
use serde_json::{self as json};
use std::ffi::{CStr, CString};
use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Starts the MM2 in a detached singleton thread.
#[no_mangle]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn mm2_main(conf: *const c_char, log_cb: extern "C" fn(line: *const c_char)) -> i8 {
    init_crash_reports();
    macro_rules! log {
        ($($args: tt)*) => {{
            let mut msg = format!("mm2_lib:{}] ", line!());
            let args = format!($($args)*);
            msg.push_str(&args);
            log_cb(msg.as_ptr() as *const c_char);
        }}
    }
    macro_rules! eret {
        ($rc: expr, $desc:expr) => {{
            log!("error {}, {:?}: {}", $rc as i8, $rc, $desc);
            return $rc as i8;
        }};
        ($rc: expr) => {{
            log!("error {}, {:?}", $rc as i8, $rc);
            return $rc as i8;
        }};
    }

    if conf.is_null() {
        eret!(StartupResultCode::InvalidParams, "Configuration is null")
    }
    let conf_cstr = CStr::from_ptr(conf);
    let conf_str = match conf_cstr.to_str() {
        Ok(s) => s,
        Err(e) => eret!(
            StartupResultCode::InvalidParams,
            format!("Configuration is not valid UTF-8: {}", e)
        ),
    };

    let conf: json::Value = match json::from_str(conf_str) {
        Ok(v) => v,
        Err(e) => eret!(
            StartupResultCode::ConfigError,
            format!("Failed to parse configuration: {}", e)
        ),
    };

    if let Err(true) = LP_MAIN_RUNNING.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed) {
        eret!(StartupResultCode::AlreadyRunning, "MM2 is already running");
    }

    // Remove the old context ID during restarts.
    CTX.store(0, Ordering::Relaxed);

    register_callback(FfiCallback::with_ffi_function(log_cb));

    let ctx_cb = &|ctx| CTX.store(ctx, Ordering::Relaxed);
    let params = LpMainParams::with_conf(conf).log_filter(None);

    let ctx = match catch_unwind(|| {
        block_on(mm2_main::lp_main(
            params,
            &ctx_cb,
            KDF_VERSION.into(),
            KDF_DATETIME.into(),
        ))
    }) {
        Ok(Ok(ctx)) => ctx,
        Ok(Err(e)) => {
            log!("MM2 initialization failed: {}", e);
            LP_MAIN_RUNNING.store(false, Ordering::Relaxed);
            return StartupResultCode::InitError as i8;
        },
        Err(err) => {
            log!("MM2 initialization panicked: {:?}", any_to_str(&*err));
            LP_MAIN_RUNNING.store(false, Ordering::Relaxed);
            return StartupResultCode::InitError as i8;
        },
    };

    // This allows us to use catch_unwind inside the lp_run thread despite MmCtx containing
    // types with interior mutability (Mutex, RwLock, etc.) that aren't UnwindSafe.
    // By passing just a numeric ID and recovering the actual context inside the
    // catch_unwind block, we satisfy the compiler's safety requirements while properly
    // handling potential panics.
    let ctx_id = match ctx.ffi_handle() {
        Ok(id) => id,
        Err(e) => {
            log!("MM2 thread setup failed: Failed to create FFI handle: {}", e);
            LP_MAIN_RUNNING.store(false, Ordering::Relaxed);
            return StartupResultCode::InitError as i8;
        },
    };

    let rc = thread::Builder::new().name("lp_run".into()).spawn(move || {
        let c = catch_unwind(move || {
            let ctx = match MmArc::from_ffi_handle(ctx_id) {
                Ok(ctx) => ctx,
                Err(err) => {
                    panic!("Failed to recover context in thread: {}", err);
                },
            };
            block_on(mm2_main::lp_run(ctx));
        });
        match c {
            Ok(_) => log!("MM2 thread completed normally"),
            Err(err) => {
                log!("MM2 thread panicked: {:?}", any_to_str(&*err));
            },
        };

        LP_MAIN_RUNNING.store(false, Ordering::Relaxed)
    });

    if let Err(e) = rc {
        LP_MAIN_RUNNING.store(false, Ordering::Relaxed);
        eret!(
            StartupResultCode::SpawnError,
            format!("Failed to spawn MM2 thread: {:?}", e)
        );
    }

    StartupResultCode::Ok as i8
}

/// Checks if the MM2 singleton thread is currently running or not.
/// 0 .. not running.
/// 1 .. running, but no context yet.
/// 2 .. context, but no RPC yet.
/// 3 .. RPC is up.
#[no_mangle]
pub extern "C" fn mm2_main_status() -> i8 {
    mm2_status() as i8
}

/// Run a few hand-picked tests.
///
/// The tests are wrapped into a library method in order to run them in such embedded environments
/// where running "cargo test" is not an easy option.
///
/// MM2 is mostly used as a library in environments where we can't simpy run it as a separate process
/// and we can't spawn multiple MM2 instances in the same process YET
/// therefore our usual process-spawning tests can not be used here.
///
/// Returns the `torch` (as in Olympic flame torch) if the tests have passed. Panics otherwise.
#[no_mangle]
pub extern "C" fn mm2_test(torch: i32, log_cb: extern "C" fn(line: *const c_char)) -> i32 {
    register_callback(FfiCallback::with_ffi_function(log_cb));

    static RUNNING: AtomicBool = AtomicBool::new(false);
    if let Err(true) = RUNNING.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed) {
        log!("mm2_test] Running already!");
        return StartupResultCode::AlreadyRunning as i32;
    }

    // #402: Stop the MM in order to test the library restart.
    let prev = if LP_MAIN_RUNNING.load(Ordering::Relaxed) {
        let ctx_id = CTX.load(Ordering::Relaxed);
        log!("mm2_test] Stopping MM instance {}…", ctx_id);
        let ctx = match MmArc::from_ffi_handle(ctx_id) {
            Ok(ctx) => ctx,
            Err(err) => {
                log!("mm2_test] Invalid CTX? !from_ffi_handle: {}", err);
                return StartupResultCode::InvalidParams as i32;
            },
        };
        let conf = json::to_string(&ctx.conf).unwrap();
        let hy_res = mm2_main::rpc::lp_commands::legacy::stop(ctx);
        let r = match block_on(hy_res) {
            Ok(r) => r,
            Err(err) => {
                log!("mm2_test] !stop: {}", err);
                return -1;
            },
        };
        if !r.status().is_success() {
            log!("mm2_test] stop status {}", r.status());
            return -1;
        }

        // Wait for `LP_MAIN_RUNNING` to flip.
        let since = now_float();
        loop {
            thread::sleep(Duration::from_millis(100));
            if !LP_MAIN_RUNNING.load(Ordering::Relaxed) {
                break;
            }
            if now_float() - since > 60. {
                log!("mm2_test] LP_MAIN_RUNNING won't flip");
                return -1;
            }
        }

        Some((ctx_id, conf))
    } else {
        None
    };

    // The global stop flag should be zeroed in order for some of the tests to work.
    let grace = 5; // Grace time for late threads to discover the stop flag before we reset it.
    thread::sleep(Duration::from_secs(grace));

    // NB: We have to catch the panic because the error isn't logged otherwise.
    // (In the release mode the `ud2` op will trigger a crash or debugger on panic
    // but we don't have debugging symbols in the Rust code then).
    let rc = catch_unwind(|| {
        log!("mm2_test] test_status…");
        common::log::tests::test_status();
    });

    if let Err(err) = rc {
        log!("mm2_test] There was an error: {}", any_to_str(&*err).unwrap_or("-"));
        return -1;
    }

    // #402: Restart the MM.
    if let Some((prev_ctx_id, conf)) = prev {
        log!("mm2_test] Restarting MM…");
        let conf = CString::new(&conf[..]).unwrap();
        let rc = unsafe { mm2_main(conf.as_ptr(), log_cb) };
        let rc = StartupResultCode::from_i8(rc).unwrap();
        if rc != StartupResultCode::Ok {
            log!("!mm2_main: {:?}", rc);
            return rc as i32;
        }

        // Wait for the new MM instance to allocate context.
        let since = now_float();
        loop {
            thread::sleep(Duration::from_millis(10));
            if LP_MAIN_RUNNING.load(Ordering::Relaxed) && CTX.load(Ordering::Relaxed) != 0 {
                break;
            }
            if now_float() - since > 60.0 {
                log!("mm2_test] Won't start");
                return StartupResultCode::InitError as i32;
            }
        }

        let ctx_id = CTX.load(Ordering::Relaxed);
        if ctx_id == prev_ctx_id {
            log!("mm2_test] Context ID is the same");
            return StartupResultCode::InvalidParams as i32;
        }
        log!("mm2_test] New MM instance {} started", ctx_id);
    }

    RUNNING.store(false, Ordering::Relaxed);
    log!("mm2_test] All done, passing the torch.");
    torch
}

#[derive(Debug, PartialEq, Primitive)]
enum StopErr {
    Ok = 0,
    NotRunning = 1,
    ErrorStopping = 2,
    StoppingAlready = 3,
}

/// Stop an MM2 instance or reset the static variables.
#[no_mangle]
pub extern "C" fn mm2_stop() -> i8 {
    let ctx = match prepare_for_mm2_stop() {
        PrepareForStopResult::CanBeStopped(ctx) => ctx,
        PrepareForStopResult::ReadyStopStatus(res) => return res as i8,
    };

    let spawner = ctx.spawner();
    let fut = finalize_mm2_stop(ctx);
    spawner.spawn(fut);

    StopErr::Ok as i8
}
