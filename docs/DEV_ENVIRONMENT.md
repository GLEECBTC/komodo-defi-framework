# Setting up the dev environment for Komodo DeFi Framework to run full tests suite

## Running native tests

1. Install Docker or Podman.
2. Install `libudev-dev` (dpkg) or `libudev-devel` (rpm) package.
3. Install protobuf compiler, so `protoc` is available in your PATH.
4. Download ZCash params files: [Windows](https://github.com/KomodoPlatform/komodo/blob/master/zcutil/fetch-params.bat),
   [Unix/Linux](https://github.com/KomodoPlatform/komodo/blob/master/zcutil/fetch-params.sh)
5. Create `.env.client` file with the following content
   ```
   ALICE_PASSPHRASE=spice describe gravity federal blast come thank unfair canal monkey style afraid
   ```
6. Create `.env.seed` file with the following content
   ```
   BOB_PASSPHRASE=also shoot benefit prefer juice shell elder veteran woman mimic image kidney
   ```
7. MacOS specific: run script (required after each reboot)
   ```shell
   #!/bin/bash
   for ((i=2;i<256;i++))
   do
       sudo ifconfig lo0 alias 127.0.0.$i up
   done
   sudo ifconfig lo0 inet6 -alias ::1
   sudo ifconfig lo0 inet6 -alias fe80::1%lo0
   ```
   Please note that you have to run it again after each reboot
8. Linux specific:
    - for Docker users:
       ```
       sudo groupadd docker
       sudo usermod -aG docker $USER
       ```
    - for Podman users:
       ```
       sudo ln -s $(which podman) /usr/bin/docker
       ```
9. Try `cargo test --all --features run-docker-tests -- --test-threads=16`.

   Warning:

   Running the tests will start several Docker containers. If any container fails to start, check for potential port conflicts with existing services on your system. For example, on MacOS, the testblockchain container (used for UTXO testing) may not start because it uses port 7000, which is also used by the MacOS AirPlay Receiver. To resolve this issue, disable AirPlay Receiver in your system settings.

   Note for MacOS users:

   The nucleusd container (and its dependent ibc-relayer container) requires host network access. However, on MacOS, Docker does not support host networking by default. To ensure the nucleusd container runs correctly, make sure to turn on the "Enable host networking" option in your Docker settings.

## Running WASM tests

1. Set up [WASM Build Environment](../docs/WASM_BUILD.md#Setting-up-the-environment)
2. Install Firefox.
3. Download [Gecko driver](https://github.com/mozilla/geckodriver/releases) for your OS
4. Set environment variables required to run WASM tests
   ```shell
   # wasm-bindgen specific variables
   export WASM_BINDGEN_TEST_TIMEOUT=180
   export GECKODRIVER=PATH_TO_GECKO_DRIVER_BIN
   # MarketMaker specific variables
   export BOB_PASSPHRASE="also shoot benefit prefer juice shell elder veteran woman mimic image kidney"
   export ALICE_PASSPHRASE="spice describe gravity federal blast come thank unfair canal monkey style afraid"
   ```
5. Run WASM tests
   - for Linux users:
   ```
   wasm-pack test --firefox --headless mm2src/mm2_main
   ```
    - for OSX users (Intel):
   ```
   CC=/usr/local/opt/llvm/bin/clang AR=/usr/local/opt/llvm/bin/llvm-ar wasm-pack test --firefox --headless mm2src/mm2_main
   ```
    - for OSX users (Apple Silicon):
   ```
   CC=/opt/homebrew/opt/llvm/bin/clang AR=/opt/homebrew/opt/llvm/bin/llvm-ar wasm-pack test --firefox --headless mm2src/mm2_main
   ```
   Please note `CC` and `AR` must be specified in the same line as `wasm-pack test mm2src/mm2_main`.

#### Running specific WASM tests

There are two primary methods for running specific tests:

*   **Method 1: Using `wasm-pack` (Recommended for browser-based tests)**

    To filter tests, append `--` to the `wasm-pack test` command, followed by the name of the test you want to run. This will execute only the tests whose names contain the provided string.

    General Example:
    ```shell
    wasm-pack test --firefox --headless mm2src/mm2_main -- <test_name_to_run>
    ```

    > **Note for macOS users:** You must prepend the `CC` and `AR` environment variables to the command if they weren't already exported, just as you would when running all tests. For example: `CC=... AR=... wasm-pack test ...`

*   **Method 2: Using `cargo test` (For non-browser tests)**

    This method uses the standard Cargo test runner with a wasm target and is useful for tests that do not require a browser environment.

    a. **Install `wasm-bindgen-cli`**: Make sure you have `wasm-bindgen-cli` installed with a version that matches the one specified in your `Cargo.toml` file.
    ```shell
    cargo install -f wasm-bindgen-cli --version <wasm-bindgen-version>
    ```

    b. **Run the test**: Append `--` to the `cargo test` command, followed by the test path.
    ```shell
    cargo test --target wasm32-unknown-unknown --package coins --lib -- utxo::utxo_block_header_storage::wasm::indexeddb_block_header_storage
    ```

PS If you notice that this guide is outdated, please submit a PR.
