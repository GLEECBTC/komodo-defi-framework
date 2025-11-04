#![cfg(not(target_arch = "wasm32"))]

#[cfg(not(any(feature = "for-tests", feature = "run-docker-tests")))]
compile_error!(concat!(
    "Integration tests for `",
    env!("CARGO_PKG_NAME"),
    "` require either the `for-tests` or the `run-docker-tests` feature."
));
