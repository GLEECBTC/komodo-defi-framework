# Contributing to Komodo DeFi Framework

We welcome contribution from everyone in the form of suggestions, bug reports, pull requests, and feedback.
Please note we have a code of conduct, please follow it in all your interactions with the project.

## Submitting feature requests

Before uploading any changes, please make sure that the test suite passes locally before submitting a pull request with your changes.

```
cargo test --all --features run-docker-tests
```

We also use [Clippy](https://github.com/rust-lang/rust-clippy) to avoid common mistakes
and we use [rustfmt](https://github.com/rust-lang/rustfmt) to make our code clear to everyone.

1. Format the code using rustfmt:
    ```shell
    cargo fmt
    ```
2. Make sure there are no warnings and errors. Run the Clippy:
    ```shell
    cargo clippy -- -D warnings
    ```
   Install cargo udeps
   ```shell
    cargo install cargo-udeps
    ```
3. Make sure there are no unused dependencies. Run the following check
   ```shell
   cargo udeps
   ```
   Install cargo deny
   ```shell
   cargo install cargo-deny
   ```
4. Make sure that no new dependencies duplicates appear. Run the following check
   ```shell
   cargo deny check bans
   ```
5. Make sure that dependencies do not have known vulnerabilities. If they do, update them.
   ```shell
   cargo deny check advisories
   ```

### Run WASM tests

1. Install Firefox.
1. Download Gecko driver for your OS: https://github.com/mozilla/geckodriver/releases
1. Run the tests
    ```
    WASM_BINDGEN_TEST_TIMEOUT=600 GECKODRIVER=PATH_TO_GECKO_DRIVER_BIN wasm-pack test --firefox --headless mm2src/mm2_main
    ```
