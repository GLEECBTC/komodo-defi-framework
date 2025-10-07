## v2.5.2-beta - 2025-10-10

### Enhancements/Fixes:

**Metrics**:
- The `memory_db` size metric that relied on `parity-util-mem::malloc_size` was removed because it intermittently segfaulted on Linux due to allocator conflicts. [#2632](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2632)

**Build and Dependency Management**:
- A CI job was added to build macOS Universal2 artifacts for KDF, this combines `x86_64-apple-darwin` and `aarch64-apple-darwin` binaries via `lipo` to produce a single binary that runs natively on both Intel and Apple Silicon. The universal binary is uploaded as `kdf_<commit>-mac-universal.zip`. [#2628](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2628)
- The `parity-util-mem` dependency was removed. [#2632](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2632)
- CI container base image was bumped to `debian:bullseye-slim` [#2534](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2534) [#2641](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2641)

---

## v2.5.1-beta - 2025-07-28

### Enhancements/Fixes:

**Wallet**:
- A comprehensive `get_private_keys` RPC was implemented to export private keys, public keys, and addresses for any configured coin without requiring activation. HD and Iguana modes with protocol-specific logic for UTXO, EVM, Tendermint, and ZHTLC coins were supported, enabling offline recovery workflows (implemented by Devin AI). [#2542](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2542)

---

## v2.5.0-beta - 2025-07-04

### Features:

**WalletConnect Integration**:
- WalletConnect v2 support for EVM and Cosmos coins was implemented, enabling wallet connection and transaction signing via the WalletConnect protocol. [#2223](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2223) [#2485](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2485) [#2508](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2508)

---

### Work in Progress (WIP) Features:

**Cosmos Network and IBC Swaps**:
- Pre-swap validation logic was implemented for maker order creation, requiring HTLC assets and healthy IBC channels on the Cosmos network, with all changes gated behind the `ibc-routing-for-swaps` feature. [#2459](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2459)
- The taker and maker order types were extended with an `order_metadata` field to carry protocol/IBC details, and cross-checks for IBC channels were added (also feature-gated), enabling both parties to validate IBC routing before a swap. [#2476](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2476)

**TRON Integration**:
- Initial groundwork for TRON integration was started, including the addition of basic structures and boilerplate code; no end-to-end functionality is yet available. [#2425](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2425)

---

### Enhancements/Fixes:

**Event Streaming**:
- Streamer IDs in the event-streaming system were strongly typed to improve type safety and code clarity. [#2441](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2441)

**Peer-to-Peer Network**:
- Hardcoded seed node IP addresses were removed from the peer-to-peer network configuration to improve maintainability. [#2439](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2439)

**Orders and Trading Protocol**:
- The minimum trading volume logic was revised to remove BTC-specific volume behavior, standardizing the calculation across all coins. [#2483](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2483)

**Tendermint / Cosmos**:
- A helper for generating internal transaction IDs for Tendermint transactions was introduced. [#2438](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2438)
- The IBC channel handler was improved to enhance safety and reliability when interacting with IBC channels. [#2298](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2298)

**Wallet**:
- Unconfirmed z-coin notes are now correctly tracked. [#2331](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2331)
- HD multi-address support for message signing was implemented, allowing message signatures from multiple derived addresses. [#2432](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2432) [#2474](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2474)
- A `delete_wallet` RPC was introduced to securely remove wallets after password confirmation, while preventing the deletion of the currently active wallet. [#2497](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2497)
- A race condition during the initialization of Trezor-based hardware wallets was resolved by ensuring the correct account and address are loaded before fetching the enabled address, preventing startup errors. [#2504](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2504)

**UTXO**:
- Validation of expected public keys for p2pk inputs was corrected, resolving an error in p2pk transaction processing. [#2408](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2408)
- Transaction fee calculation and minimum relay fee handling for UTXO coins were improved for accurate fee estimation. [#2316](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2316)

**EVM / ERC20**:
- ETH address serialization in event streaming was updated to use the `AddrToString` trait for consistency. [#2440](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2440)

**Pubkey Banning**:
- Expirable bans for pubkeys were introduced, allowing temporary exclusion of certain public keys. [#2455](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2455)

**RPC Interface**:
- A unified interface was implemented for legacy and current RPC methods. [#2450](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2450)

**DNS Resolution**:
- IP resolution logic was improved to fail only if no IPv4 address is found. [#2487](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2487)

**Database and File System**:
- More replacements of `dbdir` with `address_dir` were made as part of an ongoing improvement to database architecture. [#2398](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2398)

**Build and Dependency Management**:
- Duplicated mm2 build artifacts were removed from the build process to reduce clutter. [#2448](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2448)
- Static CRT linking was enabled for MSVC builds, improving the portability of Windows binaries. [#2464](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2464)
- The `timed-map` dependency was bumped to version `1.4.1` for improved performance and stability. [#2413](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2413) [#2481](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2481)
- The `base58` crate was removed and replaced with `bs58` throughout the codebase for consistency and security. [#2427](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2427)
- Dependencies were reorganized using the `workspace.dependencies` feature for centralized management. [#2449](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2449)

---

### Other Changes:

**Documentation**:
- Old URLs referencing atomicDEX or previous documentation pages were updated throughout the documentation. [#2428](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2428)
- A DeepWiki badge was added to the README to highlight documentation resources. [#2463](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2463)

**Core Maintenance**:
- Workspace dependencies were organized for consistent dependency management across the project. [#2449](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2449)
- A unit test was added to validate DEX fee handling for ZCoin. [#2460](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2460)
- Improved ERC20 token lookup to use platform ticker alongside contract address for proper token identification across platforms. [#2445](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2445)

---

### NB - Backwards compatibility breaking changes:

**WalletConnect/EVM Coin Activation Policy**:
- The `priv_key_policy` field for EVM coin activation now requires the new enum variant format: `"priv_key_policy": { "type": "ContextPrivKey" }`. [#2223](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2223)

**TRON/EVM Chain Specification**:
- EVM coin configurations must now include `chain_id` inside the `protocol_data` field. Legacy `chain_id` fields are deprecated. [#2425](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2425)

**mm2 Build Artifacts**:
- The `mm2` binaries have been removed from build outputs. Users must reference new artifact locations. [#2448](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2448)

**Seednode Configuration**:
- Hardcoded seed nodes were removed. KDF will no longer connect to 8762 mainnet by default without proper `seednodes` configuration. [#2439](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2439)

**IBC/Cosmos Changes**:
- The `ibc_chains` and `ibc_transfer_channels` RPC endpoints have been removed. [#2459](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2459)
- The `ibc_source_channel` field now requires numeric values only (e.g., `12` instead of `channel-12`). [#2459](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2459)

---

## v2.4.0-beta - 2025-05-02

### Features:

**Experimental Namespace**:
- Introduced experimental namespace for APIs that may undergo breaking changes in future releases [#2372](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2372)

**Event Streaming**:
- Refactored event-streaming system to support dynamic, API-driven subscription management [#2172](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2172)
- Added support for additional event types including transaction history events, swap events, and more [#2172](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2172)

**Cosmos Staking**:
- The following staking operations were added for Cosmos chains:
  - Delegation [#2322](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2322)
  - Undelegation [#2330](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2330)
  - Claiming delegation rewards [#2351](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2351) [#2373](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2373)
- Additional RPC endpoints were added for:
  - Validator data queries [#2310](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2310)
  - Delegation status and ongoing undelegations [#2377](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2377)
- Relocated staking RPCs under experimental::staking namespace with new method names [#2372](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2372)

### Enhancements/Fixes:

**Event Streaming**:
- UnknownClient error was moved to trace level [#2401](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2401)
- Initial addresses registration in UTXO balance streaming was fixed to properly track address balances when streamer is enabled [#2431](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2431)

**Peer-to-Peer Network**:
- Implemented network time synchronization validation to avoid swap failures due to node clock drift [#2255](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2255) [#2302](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2302), with additional testing coverage [#2304](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2304)
- Removed static seed node IP addresses for improved reliability [#2407](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2407)
- Improved error handling in best_orders RPC when no peers respond to orderbook requests [#2318](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2318)
- Fixed peer-to-peer backward compatibility for swap negotiations by improving serialization of pubkey fields [#2353](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2353)

**Trading Protocol Improvements**:
- Implemented zero DEX fees for v2 swaps (TPU) for KMD trading pairs [#2323](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2323)
- Added swap protocol versioning with fallback support to legacy swaps [#2324](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2324)
- Added pre-burn address outputs for collecting 25% of taker DEX fees:
  - UTXO swaps (both v1 and v2 protocols) [#2112](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2112)
  - Cosmos and ARRR swaps (legacy protocol only) [#2112](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2112)
- Fixed payment spend and secret extraction logic in swaps v2 [#2261](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2261)
- Removed unnecessary state during ETH funding validation in swaps v2 [#2334](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2334)
- Allowed skipping unnecessary P2P message handling in ETH swaps v2 [#2359](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2359)
- Improved swap robustness by checking for existing maker/taker payments before timeout validation [#2283](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2283)
- Fixed memory leak issue in legacy swaps tracking (running_swap) [#2301](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2301)
- Added `is_success` field to legacy swap status response, making success state more explicit [#2371](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2371)

**Tendermint/Cosmos Improvements**:
- Improved transaction query reliability using standardized TxSearchRequest [#2384](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2384)
- Added token transaction history support [#2404](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2404)
- Fixed unhandled IBC and HTLC events [#2385](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2385)

**Wallet Improvements**:
- Added an RPC to change mnemonic passwords [#2317](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2317)
- Enabled storage and usage of non-BIP39 mnemonics [#2312](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2312)
- Fixed hardware-wallet context initialization for UTXO withdrawals [#2333](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2333)
- Added validation to restrict wallet names to alphanumeric characters, dash, and underscore [#2400](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2400)
- Changed wallet file extension from .dat to .json to better reflect content [#2400](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2400)
- Implemented optional password strength validation for mnemonic encryption [#2400](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2400)

**FIRO Integration**:
- Added support for FIRO Spark verbose transaction details [#2396](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2396)

**Pirate/ARRR Integration**:
- Dockerized Zombie/Pirate tests for improved test environment reliability [#2374](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2374)

**Database and File System Improvements**:
- Improved database architecture with context functions and global DB usage has started in [#2378](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2378), this is still under development under the new-db-arch compilation flag.
- Fixed file filtering logic to exclude directories [#2364](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2364)

**RPC Enhancements**:
- Implemented dynamic RPC port allocation (rpcport: 0) allowing automatic selection of available ports [#2342](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2342)

**NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)**:
- Fixed `update_nft` to work correctly with HD wallets using the enabled address [#2386](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2386)
- Fixed `withdraw_nft` method to work correctly in HD mode [#2424](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2424)

**Simple Maker Bot [#1065](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1065)**:
- Added LiveCoinWatch price provider and removed deprecated Nomics provider [#2416](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2416)

**UTXO Improvements**:
- Added support for parsing taproot output address types from `blockchain.transaction.get` verbose transactions [#2423](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2423)

**Qtum Delegation**:
- Standardized parameter naming by using `validator_address` for QTUM delegation similar to Cosmos [#2419](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2419)

**Security Fixes**:
- Fixed potential panics in hash processing by enforcing fixed-size arrays and proper length validation [#2279](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2279)
- Improved security of key derivation by validating key material length [#2356](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2356)
- Ensured consistent Argon2 parameters for wallet encryption/decryption [#2360](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2360)
- Fixed path traversal vulnerability in wallet file handling [#2400](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2400)

### Other Changes:

**Code and Dependencies**:
- Added default implementations for protocol-specific SwapOps functions [#2354](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2354)
- ETH address displaying now uses a generic trait [#2348](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2348)
- Removed unnecessary Arc wrappers from Ethereum websocket implementation [#2291](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2291)
- Updated dependencies:
  - Replaced deprecated instant dependency [#2391](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2391)
  - Completed migration to timed-map crate [#2247](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2247) [#2306](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2306) [#2308](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2308)
  - Bumped libp2p from k-0.52.11 to k-0.52.12 to fix iOS platform build issues [#2326](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2326)

**Build and Testing Improvements**:
- Rewrote main build script for clarity/stability and to eliminate cache invalidation [#2319](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2319)
- mm2_main error is now unified across native and wasm [#2389](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2389)
- mm2_main in wasm now returns a js promise by making it async [#2389](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2389)
- A lot of unstable tests were made more stable in [#2365](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2365)
- Added additional Sepolia testnet nodes for improved test coverage [#2358](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2358)
- Fixed failing Electrum protocol version test [#2412](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2412)
- Updated Docker build configuration for WASM to fix dependency version issues [#2294](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2294)
- Fixed WASM build by adding test-ext-api feature to required toml files [#2295](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2295)
- Improved CI performance with proper Rust caching implementation [#2303](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2303)
- Updated broken RPC link in Cosmos tests [#2305](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2305)
- Unlocked wasm-pack version constraints in CI workflows and Docker builds [#2307](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2307)
- Fixed mm2_p2p module development build to support individual module testing [#2311](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2311)
- Added Cargo.lock validation to CI process to prevent lockfile inconsistencies [#2309](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2309)
- Improved branch naming flexibility by allowing feature-specific patterns like `feat/swapstatus-is-success` [#2371](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2371)
- Fixed formatting and linting job failures by correcting the syntax for rustup component installation [#2390](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2390)

**NB - Backwards compatibility breaking changes:**
- Event streaming model changed from static configuration to API-driven subscription [#2172](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2172)

## v2.3.0-beta - 2024-12-19

### Features:
- **1inch Integration**:
  - Initial code to connect to 1inch Liquidity Routing API (LRAPI) provider was added along with two new RPCs for 1inch classic swap API [#2222](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2222)
  - New 'approve' and 'allowance' RPCs for ERC20 tokens were also added [#2222](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2222)

### Enhancements/Fixes:
- **Error Handling**:
  - KDF now checks main files (config/coins/etc.) before reading them to prevent potential panics [#2288](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2288)
  - Special character restrictions (<, >, &) were removed from RPC request bodies that were incorrectly blocking valid password characters in the get_mnemonic RPC call [#2287](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2287)
- **Dependencies**:
  - Removed unnecessary reliance on core [#2289](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2289)
  - Bumped libp2p dependency to k-0.52.11 for security reasons [#2296](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2296)
- **Performance**:
  - Replaced GStuff constructible with OnceCell for better performance [#2267](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2267)

## v2.2.0-beta - 2024-11-22

**Features:**
- Connection Healthcheck
  - Connection healthcheck implementation for peers was introduced. [#2194](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2194)
- Custom Tokens Activation
  - Support for enabling custom EVM (ERC20, PLG20, etc..) tokens without requiring them to be in the coins config was added. [#2141](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2141)
    - This allows users to interact with any ERC20 token by providing the contract address.

**Enhancements/Fixes:**
- Trading Protocol Upgrade [#1895](https://github.com/KomodoPlatform/atomicDEX-API/issues/1895)
  - EVM TPU taker methods were implemented and enhancements were made to ETH docker tests. [#2169](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2169)
  - EVM TPU maker methods were implemented. [#2211](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2211)
- NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - Refund methods for NFT swaps were completed. [#2129](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2129)
  - `token_id` field was added to the tx history primary key. [#2209](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2209)
- Graceful Shutdown
  - CTRL-C signal handling with graceful shutdown was implemented. [#2213](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2213)
- Seed Management [#1939](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1939)
  - A new `get_wallet_names` RPC was added to retrieve information about all wallet names and the currently active one. [#2202](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2202)
- Cosmos Integration [#1432](https://github.com/KomodoPlatform/atomicDEX-API/issues/1432)
  - Cosmos tx broadcasting error was fixed by upgrading cosmrs to version 15. [#2238](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2238)
  - Cosmos transaction history implementation was incorrectly parsing addresses (using the relayer address instead of the cross-chain address) from IBC transactions. The address parsing logic was fixed in [#2245](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2245)
- Order Management
  - Cancel order race condition was addressed using time-based cache. [#2232](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2232)
- Swap Improvements
  - A legacy swap issue was resolved where taker spent maker payment transactions were sometimes incorrectly marked as successful when they were actually reverted or not confirmed, particularly in EVM-based swaps. [#2199](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2199)
    - Two new events were added: "MakerPaymentSpendConfirmed" and "MakerPaymentSpendConfirmFailed"
  - A fix was introduced where Takers don't need to confirm their own payment as they can wait for the spending of it straight away. [#2249](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2249)
    - This invalidates this fix [#1442](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1442), a better solution will be introduced where taker rebroadcasts their transaction if it's not on the chain.
  - A fix was introduced for recover funds for takers when the swap was marked as unsuccessful due to the maker payment spend transaction not being confirmed. [#2242](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2242)
    - The required confirmations from coin config for taker/maker payment spend are now used instead of using 1 confirmation max. This is because some chains require more than 1 confirmation for finality, e.g. Polygon.
- Swap watchers [#1431](https://github.com/KomodoPlatform/atomicDEX-API/issues/1431)
  - Taker fee validation retries now work the same way as for makers. [#2263](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2263)
- Electrum Client
  -  Electrum client was refactored to add min/max connection controls, with server priority based on list order. [#1966](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1966)
    - Electrum client can now operate in single-server mode (1,1) to reduce resource usage (especially beneficial for mobile) or multi-server (legacy) mode for reliability.
    - Higher priority servers automatically replace lower priority ones when reconnecting during periodic retries or when connection count drops below minimum.
- Coins Activation
  - EVM addresses are now displayed in full in iguana v2 activation response. [#2254](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2254)
- HD Wallet [#1838](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1838)
  - Balance is now returned as `CoinBalanceMap` for both UTXOs and QTUM. [#2259](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2259)
    - This is to return the same type/json across all coins for GUIs since EVM uses `CoinBalanceMap`.
  - EVM addresses are displayed in full in `get_new_address` response after [#2264](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2264)
- RPC Service
  - A fix was introduced to run rpc request futures till completion in [#1966](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1966)
    - This ensures RPC request futures complete fully even if clients disconnect, preventing partial state updates and maintaining data consistency.
- Security Enhancements
  - Message lifetime overflows were added to prevent creating messages for proxy with too long lifetimes. [#2233](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2233)
  - Remote files are now handled in a safer way in CI. [#2217](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2217)
- Build Process
  - `wasm-opt` overriding was removed. [#2200](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2200)
- Escaped response body in native RPC was removed. [#2219](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2219)
- Creation of the all-zeroes dir on KDF start was stopped. [#2218](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2218)
- OPTIONS requests to KDF server were added. [#2191](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2191)

**Removals:**
- Solana Support [#1085](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1085)
  - Solana implementation was removed until it can be redone using the latest Solana SDK. [#2239](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2239)
- Adex-CLI [#1682](https://github.com/KomodoPlatform/atomicDEX-API/issues/1682)
  - adex-cli was deprecated pending work on a simpler, more maintainable implementation. [#2234](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2234)

**Other Changes:**
- Documentation
  - Issue link in README was updated. [#2227](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2227)
  - Commit badges were updated to use dev branch in README. [#2193](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2193)
  - Leftover subcommands were removed from help message. [#2235](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2235) [#2270](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2270)
- Code Structure
  - lib.rs was replaced by mm2.rs as the root lib for mm2_main. [#2178](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2178)
- Code Improvements
  - P2P feature was added to mm2_net dependency to allow the coins crate to be compiled and tested independently. [#2210](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2210)
  - Coins mod clippy warnings in WASM were fixed. [#2224](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2224)
  - Nonsense CLI arguments were removed. [#2216](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2216)
- Tests
  - Tendermint IBC tests were fixed by preparing IBC channels inside the container. [#2246](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2246)
  - `.wait()` usage was replaced with `block_on` in tests to ensure consistent runtime usage, fixing issues with tokio TCP streams in non-tokio runtimes. [#2220](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2220)
  - Debug assertions for tests were enabled. [#2204](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2204)
  - More Sepolia test endpoints were added in [#2262](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2262)

**NB - Backwards compatibility breaking changes:**
- RPC Renaming
  - `get_peers_info` RPC was renamed to `get_directly_connected_peers`. [#2195](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2195)
- Cosmos Integration [#1432](https://github.com/KomodoPlatform/atomicDEX-API/issues/1432)
  - Updates to Tendermint activation payloads:
    - 'rpc_urls' field (previously a list of plain string values) is replaced with 'nodes' (a list of JSON objects). [#2173](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2173)
- Komodo DeFi Proxy
  - All RPC methods fields controlling komodo-defi-proxy are renamed to 'komodo_proxy', affecting various activations, including ETH/EVM. [#2173](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2173)


## v2.1.0-beta - 2024-07-31

**Features:**
- Seed Management [#1939](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1939)
  - Seed generation, encryption, and storage features were introduced, including a new `get_mnemonic` API. [#2014](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2014)
- A new `sign_raw_transaction` rpc was added for UTXO and EVM coins, this will facilitate air-gapped wallet implementation in the future. [#1930](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1930)

**Enhancements/Fixes:**
- Event Streaming [#1901](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1901)
  - Balance event streaming for Electrum clients was implemented. [#2013](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2013)
    - Balance events for UTXOs were introduced.
    - Electrum notification receiving bug was fixed.
  - Balance event streaming for EVM was added. [#2041](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2041)
  - Error events were introduced. [#2041](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2041)
  - Heartbeats were introduced to notify about streaming channel health. [#2058](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2058)
  - Balance event streaming for ARRR/Pirate was added. [#2076](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2076)
- Trading Protocol Upgrade [#1895](https://github.com/KomodoPlatform/atomicDEX-API/issues/1895)
  - *Important note:* Seednodes update is needed to support and rebroadcast new swap protocol messages. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - WASM storage for upgraded swaps introduced. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - Migration of old swaps data was added. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - Swaps now automatically kickstart on MM2 reload. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - File lock for swaps added, preventing the same swap from starting in different processes. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - `my_swap_status`, `my_recent_swaps` V2 RPCs introduced. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - Upgraded swaps data now accessible through V1 RPCs. [#2015](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2015)
  - Locked amount handling for UTXO swaps implemented. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
  - Conditional wait for maker payment confirmation was added before signing funding tx spend preimage on taker's side. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
  - `active_swaps` V2 RPC introduced. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
  - Handling `accept_only_from` for swap messages (validation of the sender) was implemented. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
  - `swap_uuid` for swap v2 messages was added to avoid reuse of the messages generated for other swaps. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
  - Maker payment immediate refund path handling was implemented. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
- KMD Burn [#2010](https://github.com/KomodoPlatform/komodo-defi-framework/issues/2010)
  - KMD dex fee burn for upgraded swaps was added. [#2046](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2046)
- Hardware Wallet [#964](https://github.com/KomodoPlatform/atomicDEX-API/issues/964)
  - Trezor now supports SegWit for withdrawals. [#1984](https://github.com/KomodoPlatform/atomicDEX-API/pull/1984)
  - Trezor support was added for EVM coins/tokens using task manager activation methods. [#1962](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1962)
  - Support for unsigned Tendermint transactions using Ledger's Keplr extension was added, excluding HTLC transactions and swap operations. [#2148](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2148)
- NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - A new `clear_nft_db` RPC for NFT data management was added. This enables selective (based on a chain) or complete NFT DB data clearance. [#2039](https://github.com/KomodoPlatform/atomicDEX-API/pull/2039)
  - NFT can now be enabled using `enable_eth_with_tokens` or `enable_nft`, similar to `enable_erc20`. [#2049](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2049)
  - NFT swaps V2 POC was shown, which includes a NFT maker payment test using the dockerized Geth dev node. [#2084](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2084)
  - `komodo-defi-proxy` support for NFT feature was added. [#2100](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2100)
  - Additional checks were added for malicious `token_uri` links. [#2100](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2100)
  - `clear_all` parameter in `clear_nft_db` RPC is now optional (default: `false`). [#2100](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2100)
- WASM Worker
  - Improved environment detection to ensure the correct method is used for accessing the IndexedDB factory, accommodating both window and worker contexts. [#1953](https://github.com/KomodoPlatform/atomicDEX-API/pull/1953), [#2131](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2131)
  - SharedWorker support was added, allowing any worker path in `event_stream_configuration` with a default to `event_streaming_worker.js`. [#2080](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2080)
- Simple Maker Bot [#1065](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1065)
  - Maker bot was updated to support multiple price URLs. [#2027](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2027)
  - `testcoin` was added to provider options to allow testing the maker bot using test chains assets. [#2161](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2161)
- IndexedDB
  - Cursor implementation was fixed, ensuring stable iteration over items. [#2028](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2028)
  - Advanced cursor filtering features were added, including limit, offset, and a fix for `where_` condition/option. [#2066](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2066)
- Swap Stats DB
  - `stats_swaps` table now includes GUI and MM2 version data used for a swap. [#2061](https://github.com/KomodoPlatform/atomicDEX-API/pull/2061)
- P2P Layer
  - Added `max_concurrent_connections` to MM2 config to control the maximum number of concurrent connections for Gossipsub. [#2063](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2063)
- ARRR/Pirate [#927](https://github.com/KomodoPlatform/komodo-defi-framework/issues/927)
  - ARRR/Pirate wallet and Dex operations now work in browser environments / WASM. [#1957](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1957), [#2077](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2077)
  - Syncing and activation improvements were made, including stopping sync status after main sync and refining `first_sync_block` handling. [#2089](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2089)
- EVM Transport
  - ETH websocket transport was introduced. `komodo-defi-proxy` signed messages were also supported for websocket transport. [#2058](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2058)
- Tendermint integration [#1432](https://github.com/KomodoPlatform/atomicDEX-API/issues/1432)
  - Nucleus chain support was introduced as an alternative HTLC backend to Iris. [#2079](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2079)
  - Tendermint fee calculation was fixed to use `get_receiver_trade_fee` in platform coin. [#2106](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2106)
  - Pubkey-only mode for Tendermint protocol was introduced, allowing use of any external wallet for wallet and swap operations. [#2088](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2088)
  - `ibc_withdraw` RPC was removed, and `withdraw` was refactored to support IBC transfers by automatically finding IBC channels. [#2088](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2088)
  - Transaction history handling was enhanced to support base64 encoded transaction values for Cosmos-based networks, preventing missing transactions in the history table. [#2133](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2133)
  - The precision of max amount handling was improved for Tendermint withdraw operations by simulating the transaction and removing the estimated fee. [#2155](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2155)
  - Account sequence numbers are now resolved locally, incorrect sequence numbers from cached responses are also avoided. [#2164](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2164)
- HD Wallet [#1838](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1838)
  - Full UTXO and EVM HD wallet functionalities were implemented. [#1962](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1962)
- Swap watchers [#1431](https://github.com/KomodoPlatform/atomicDEX-API/issues/1431)
  - UTXO swaps were fixed to apply events that occurred while the taker was offline, such as maker spending or watcher refunding the taker payment. [#2114](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2114)
- Fees Improvements [#1848](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1848)
  - EIP-1559 gas fee estimator and RPCs were added for ETH, including priority fee support for withdrawals and swaps, and improved gas limit for swap transactions. [#2051](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2051)
  - `gas_limit` parameter can be used in coins config to override default gas limit values. [#2137](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2137)
  - Default `gas_limit` values now ensure that Proxied ERC20 tokens have sufficient gas. [#2137](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2137)
- Rust Toolchain [#1972](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1972)
  - Toolchain was upgraded to Rust toolchain version 1.72 nightly (nightly-2023-06-01). [#2149](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2149)
  - rust-analyzer was added into the workspace toolchain. [#2179](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2179)
- CI Builds
  - MacOS builds for Apple Silicon are now provided through the CI pipeline. [#2163](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2163)
- Miscellaneous
  - BCH block header deserialization was fixed to match BTC's handling of `KAWPOW` version headers. [#2099](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2099)
  - Implemented root application directory `.kdf` under `$HOME` to consolidate all runtime files, enhancing user experience by following standard UNIX practices. [#2102](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2102)
  - Memory usage was improved a bit through preallocation optimizations. [#2098](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2098)
  - Swaps and orders file handling was enhanced to use `.tmp` files to avoid concurrent reading/writing issues. [#2118](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2118)
  - UTXO P2PK balance is now shown as part of the P2PKH/Legacy address balance and can be spent in withdrawals and swaps. [#2053](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2053)
  - `wallet-only` restriction was removed from `max_maker_vol` RPC, enabling its use for wallet-only mode assets. [#2153](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2153)

**NB - Backwards compatibility breaking changes:**
- Renamed `mm2` binaries to `kdf`, while providing backward-compatible copies with `mm2` naming; WASM binaries use `kdf` naming only, which is a breaking change. [#2126](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2126)


## v2.0.0-beta - 2023-12-15
**Features:**
- KMD Burn [#2010](https://github.com/KomodoPlatform/komodo-defi-framework/issues/2010)
  - Burn 25% of taker fee when paid in KMD [#2006](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2006).
- Trading Protocol Upgrade [#1895](https://github.com/KomodoPlatform/atomicDEX-API/issues/1895)
  - Implement successful swaps v2 of UTXO to UTXO coins in [#1958](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1958).
  - Add Swaps V2 message exchange using Protobuf in [#1958](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1958).
  - Storing upgraded swaps data to SQLite DB was partially implemented in [#1980](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1980).
  - Protocol enhancement for UTXO coins by adding one more funding tx for taker, which can be reclaimed immediately if maker back-outs was implemented in [#1980](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1980).
- Event Streaming [#1901](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1901)
  - Streaming channels using mpsc and SSE to send data to clients continuously was implemented in [#1945](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1945).
  - NETWORK event was implemented to show this new functionality in [#1945](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1945).
  - Wasm event streaming using workers was added in [#1978](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1978).
  - `COIN_BALANCE` events for Tendermint Protocol were added in [#1978](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1978).

**Enhancements/Fixes:**
- Network Enhancements:
  - P2P layer now uses the latest stable libp2p version in [#1878](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1878).
  - `7777` network was deprecated in [#2020](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2020).
  - Seednodes for `netid` `8762` were updated in [#2024](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2024).
  - `libp2p-yamux` now uses yamux `v0.13` (new version) by default and fall back to yamux `v0.12` (old version) when setting any configuration options in [#2030](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2030).
  - The backpressure buffer cap was increased from `25` to `256` in [#2030](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2030).
  - New protocol version (Version2) was used for peer exchange and request-response behaviours in [#2030](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2030).
  - Network information is now written to stdout to find mm2 ports easily after [#2034](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2034).
- NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - `exclude_spam` and `exclude_phishing` params were added for `get_nft_list` and `get_nft_transfers` RPCs in [#1959](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1959).
  - `nft_cache_db` was added in `NftCtx` for non wasm targets in [#1989](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1989).
  - `AsyncConnection` structure that can be used as async wrapper for sqlite connection was added in [#1989](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1989).
  - `async_sqlite_connection` field was added to `MmCtx` in [#1989](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1989).
  - Spam transfers with empty meta no longer update after [#1989](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1989).
- ARRR/Pirate:
  - ARRR infrastructure for lightwallet servers uses a fork of lightwalletd, the grpc service was renamed `from cash.z.wallet.sdk.rpc` to `pirate.wallet.sdk.rpc` to use the lightwalletd fork in [#1963](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1963).
  - Previous blocks/wallet sync will be resumed if `sync_params` are not provided after restart in [#1967](https://github.com/KomodoPlatform/atomicDEX-API/issues/1967).
- Adex-CLI [#1682](https://github.com/KomodoPlatform/atomicDEX-API/issues/1682)
  - Exact dependency versions of `hyper-rustls`, `rustls` and other deps was set in [#1956](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1956).
  - A warning was added on insecure cli configuration file mode in [#1956](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1956).
- Storable State Machine abstraction was added while having few changes to existing state machines in [#1958](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1958).
- EVM web3 requests timeout was reduced to 20s in [#1973](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1973).
- Fixed 0.0001 min threshold for TakerFee was removed in [#1971](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1971).
- The minimum trading volume for evm and tendermint was changed to be the smallest possible amount of the coin in [#1971](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1971).
- Minimum trading price is reduced to be any value above 0 in [#1971](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1971).
- Cryptocondition script type was added to utxo transactions in [#1991](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1991).
- On response error the next web3 node is tried in [#1998](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1998).
- Watchtower taker-side restart bug was fixed in [#1908](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1908).
- 'version' method was added to `PUBLIC_METHODS` that require no login in [#2001](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2001).
- `rpcport` value can now accept a string after [#2026](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2026).
- An additional `PRICE_ENDPOINTS` url which is a cached copy of `https://prices.komodian.info/api/v2/tickers` and is updated every minute was added in [#2032](https://github.com/KomodoPlatform/komodo-defi-framework/pull/2032).

**NB - Backwards compatibility breaking changes:**
- `7777` Network deprecation and the upgrade to a new p2p layer breaks compatibility with previous versions of Komodo DeFi Framework. Connections between nodes/clients running an older version of Komodo DeFi Framework and nodes/clients running this latest version will not be possible. To avoid this, all nodes/clients must be upgraded to the latest version of Komodo DeFi Framework.
- Because of KMD burn of a part of the taker fee, the taker fee outputs for any `coin/KMD` swap are changed and makers running older versions will not be able to validate the taker fee, this will cause the swap to fail. This case will never happen anyway because older versions will not be able to connect to this latest version due to the network upgrade.
- Because of the removal of the fixed 0.0001 min threshold for TakerFee, taker fee validation will also fail for these cases. Again, this case will never happen as the previous case.


## v1.0.7-beta - 2023-09-08
**Features:**
- Trading Protocol Upgrade [#1895](https://github.com/KomodoPlatform/atomicDEX-API/issues/1895)
   - SwapOpsV2 trait was added containing methods of the new protocol (WIP) in [#1927](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1927)
   - SwapOpsV2 was implemented for UtxoStandardCoin in [#1927](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1927)
   - Dockerized integration tests added, sending and spending/refunding "dex fee + premium" UTXO in [#1927](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1927)
- HD Wallet [#1838](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1838)
   - Global enabling of an account'/change/address_index path for all coins using hd_account_id config parameter was replaced by enable_hd which is a bool that defaults to false in [#1933](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1933)
   - path_to_address parameter was added to coins activation requests to set the default account'/change/address_index path that will be used for swaps. If not provided, the default will be 0'/0/0 in [#1933](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1933)
   - HD withdrawal from any account'/change/address_index path was implemented for UTXO, EVM and Tendermint coins in [#1933](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1933)
- Pirate Integration [#927](https://github.com/KomodoPlatform/komodo-defi-framework/issues/927)
   - ARRR synchronization now supports using a specific start date. This allows users to specify a specific date as the starting point for synchronization as a substitute for the checkpoint block from config or syncing from the first block [#1922](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1922)

**Enhancements/Fixes:**
- Adex-CLI [#1682](https://github.com/KomodoPlatform/atomicDEX-API/issues/1682)
   - The file permissions of the cli config file is now set to 660 in unix to disallow reading by other users [#1913](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1913)
   - Activation types have been introduced to prevent malicious substitution of them in the activation scheme file [#1912](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1912)
   - HTTPS connection support was added in [#1910](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1910)
   - Activation scheme was changed so the related data types were refactored to be fit for it in [#1938](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1938)
- PoSV coins withdrawal issue was fixed. The issue was a missing n_time field in the generated transaction. The fix now correctly considers when n_time is required, and the rawtransaction can be broadcasted [#1925](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1925)
- Latest relayer channel is now used for tendermint test [#1929](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1929)
- Price urls were updated in [#1928](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1928)
- NFT transactions that transfer multiple NFT tokens were fixed in db, log_index is now used as part of the transfers history table primary key [#1926](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1926)
- State machine was refactored as a preparation step for StorableStateMachine pattern extension in [#1927](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1927)
- A fix was introduced to use kmd rewards for fees if change + interest is below dust threshold in [#1944](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1944)
- Debug info was removed from release binary to reduce the file size in [#1954](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1954)
- Failing tests due to BCHD were ignored in [#1955](https://github.com/KomodoPlatform/komodo-defi-framework/pull/1955)


## v1.0.6-beta - 2023-07-24

**Features:**
- Komodo DeFi Framework was introduced in [#1903](https://github.com/KomodoPlatform/atomicDEX-API/issues/1903)
   - The project/repo was renamed from AtomicDEX-API to Komodo-DeFi Framework
   - The readme file, logo, links, and some references in the documentation were updated to reflect the new name/brand
   - CI was updated to use the new name where it's needed.
- Swap watcher nodes [#1431](https://github.com/KomodoPlatform/atomicDEX-API/issues/1431)
  - Using watcher nodes for swaps were enabled by default for UTXO coins in [#1859](https://github.com/KomodoPlatform/atomicDEX-API/pull/1859)
    - `use_watchers` configuration was set to true by default. It was later disabled in [#1897](https://github.com/KomodoPlatform/atomicDEX-API/pull/1897) due to this issue [#1887](https://github.com/KomodoPlatform/atomicDEX-API/issues/1887) 
    - All nodes doing a swap will broadcast a watcher message after the taker payment is sent if the swapped coins are supported by watchers (currently only UTXO). This was also disabled in [#1897](https://github.com/KomodoPlatform/atomicDEX-API/pull/1897) due to this issue [#1887](https://github.com/KomodoPlatform/atomicDEX-API/issues/1887)
    - This update also fixes an issue that caused nodes to broadcast two consecutive watcher messages after the taker payment was sent.
- NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - Cache support was added for sqlite (non-wasm targets) in [#1833](https://github.com/KomodoPlatform/atomicDEX-API/pull/1833)
  - IndexedDb support for wasm was added in [#1877](https://github.com/KomodoPlatform/atomicDEX-API/pull/1877)
  - DB unit tests were added in [#1877](https://github.com/KomodoPlatform/atomicDEX-API/pull/1877)
  - Handling of `bafy` in IPFS Moralis links in a correct way was done in [#1877](https://github.com/KomodoPlatform/atomicDEX-API/pull/1877)
  - `get_uri_meta` function was added to optimize the retrieval of `UriMeta` from `token_uri` and `metadata` in [#1877](https://github.com/KomodoPlatform/atomicDEX-API/pull/1877)
  - `protect_from_spam` feature was added to redact URLs in specific fields and flag them as possible spam in [#1877](https://github.com/KomodoPlatform/atomicDEX-API/pull/1877)
  - Address is now used instead of string in NFT and transaction objects in [#1914](https://github.com/KomodoPlatform/atomicDEX-API/pull/1914)
  - `guard: Arc<AsyncMutex<()>>` from struct `NftCtx` is added to lock nft functions which uses db in [#1914](https://github.com/KomodoPlatform/atomicDEX-API/pull/1914)
  - IndexedDB Cursor collect method was used to fix uncaught Error in [#1914](https://github.com/KomodoPlatform/atomicDEX-API/pull/1914)
- HTTPS support was added for the RPC server in [#1861](https://github.com/KomodoPlatform/atomicDEX-API/pull/1861)
- Adex-CLI [#1682](https://github.com/KomodoPlatform/atomicDEX-API/issues/1682)
  - New commands `enable`, `get-enabled`, `orderbook`,`sell`, `buy` were added to adex-cli in [#1768](https://github.com/KomodoPlatform/atomicDEX-API/pull/1768)

**Enhancements/Fixes:**
- Some RUSTSEC advisories where resolved in [#1853](https://github.com/KomodoPlatform/atomicDEX-API/pull/1853)
- ARRR/ZCOIN code was refactored to be compiled in WebAssembly (WASM) in [#1805](https://github.com/KomodoPlatform/atomicDEX-API/pull/1805)
  - The PR for this paves the way for subsequent implementation of the empty/todo functions related to WASM storage and other functionalities.
- Orderbook response now returns the right age for the age field, this was fixed in [#1851](https://github.com/KomodoPlatform/atomicDEX-API/pull/1851)
- A bug that caused `best_orders` rpc to return `is_mine: false` for the user's orders was fixed in [#1846](https://github.com/KomodoPlatform/atomicDEX-API/pull/1846)
  - An optional parameter `exclude_mine` was also added to the `best_orders` request that allows users to exclude their own orders from the response.
  - `exclude_mine` defaults to false to maintain the same behaviour before the PR.
- Watchtower integration tests were moved to the new ethereum testnet and the ignore attributes were removed in [#1846](https://github.com/KomodoPlatform/atomicDEX-API/pull/1846)
  - The PR also adds a new test case for watcher rewards.
  - It also fixes the unstable `send_and_refund_eth_payment`, `send_and_refund_erc20_payment`, `test_nonce_lock` and `test_withdraw_and_send tests` tests that were failing due to concurrency issues.
- Infrastructure DNS rotation for default seednodes was done in [#1868](https://github.com/KomodoPlatform/atomicDEX-API/pull/1868)
- Price endpoints were updated in [#1869](https://github.com/KomodoPlatform/atomicDEX-API/pull/1869)
- A fix removed the passed config string from the error logs during mm2 initialization if there was a deserialization error was done in [#1872](https://github.com/KomodoPlatform/atomicDEX-API/pull/1872)
- The time needed for CI completion was reduced by caching the downloaded dependencies in [#1880](https://github.com/KomodoPlatform/atomicDEX-API/pull/1880)
- Label validation on PRs was added. This validation will only succeed if one of the following labels is used but not both: `under review` or `in progress` [#1881](https://github.com/KomodoPlatform/atomicDEX-API/pull/1881)
- `orderbook` mod of adex-cli was refactored by moving it from the internal `response_handler` to its appropriate folder, enhancing code organization and clarity in [#1879](https://github.com/KomodoPlatform/atomicDEX-API/pull/1879)
- A bug was fixed for adex-cli to allow starting if configuration does not exist in [#1889](https://github.com/KomodoPlatform/atomicDEX-API/pull/1889)
- IBC and standard withdrawals for Cosmos now allow users to specify the gas price and gas limit for each transaction [#1894](https://github.com/KomodoPlatform/atomicDEX-API/pull/1894)
- A fix was introduced to adex-cli to allow starting mm2 from cli under regular user in macOS [#1856](https://github.com/KomodoPlatform/atomicDEX-API/pull/1856)
- The repo logo was updated to be visible in GitHub light theme in [#1904](https://github.com/KomodoPlatform/atomicDEX-API/issues/1904)
- A CI job was added to check if mm2 version was bumped before merging any pull request to main in [#1899](https://github.com/KomodoPlatform/atomicDEX-API/issues/1899)
- All CI tests now run with the `--no-fail-fast` flag, allowing other tests to proceed despite any failures [#1907](https://github.com/KomodoPlatform/atomicDEX-API/issues/1907)
- Index out of bounds errors in the `tx_details_by_hash` functions was fixed in [#1915](https://github.com/KomodoPlatform/atomicDEX-API/issues/1915)
- Adex-CLI `test_activation_scheme` was fixed by removing the old file in [#1920](https://github.com/KomodoPlatform/atomicDEX-API/issues/1920)


## v1.0.5-beta - 2023-06-08

**Features:**
- NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - UriMeta was added to get info from token uri, status and metadata in nft tx history [#1823](https://github.com/KomodoPlatform/atomicDEX-API/pull/1823)

**Enhancements/Fixes:**
- Deprecated `wasm-timer` dependency was removed from mm2 tree [#1836](https://github.com/KomodoPlatform/atomicDEX-API/pull/1836)
- `log`, `getrandom` and `wasm-bindgen` dependencies were updated to more recent versions that are inline with the latest libp2p upstream [#1837](https://github.com/KomodoPlatform/atomicDEX-API/pull/1837)
- A CI lint pipeline was added that validates pull request titles to ensure that they comply with the conventional commit specifications [#1839](https://github.com/KomodoPlatform/atomicDEX-API/pull/1839)
- KMD AUR were reduced from 5% to 0.01% starting at `nS7HardforkHeight` to comply with [KIP-0001](https://github.com/KomodoPlatform/kips/blob/main/kip-0001.mediawiki) [#1841](https://github.com/KomodoPlatform/atomicDEX-API/pull/1841)


## v1.0.4-beta - 2023-05-23

**Features:**
- NFT integration [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - Proxy support was added [#1775](https://github.com/KomodoPlatform/atomicDEX-API/pull/1775)

**Enhancements/Fixes:**
- Some enhancements were done for `enable_bch_with_tokens`,`enable_eth_with_tokens`,`enable_tendermint_with_assets` RPCs in [#1762](https://github.com/KomodoPlatform/atomicDEX-API/pull/1762)
  - A new parameter `get_balances` was added to the above methods requests, when this parameter is set to `false`, balances will not be returned in the response. The default value for this parameter is `true` to ensure backward compatibility.
  - Token balances requests are now performed concurrently for the above methods.
- Swap watcher nodes [#1750](https://github.com/KomodoPlatform/atomicDEX-API/pull/1750)
  - PoC for ETH/UTXO and ERC20/UTXO swaps with rewards
  - Improved protocol to let only the taker pay the reward
- Add passive parent coin state for keeping tokens active when platform is disabled [#1763](https://github.com/KomodoPlatform/atomicDEX-API/pull/1763)
- Optimize release compilation profile for mm2 [#1821](https://github.com/KomodoPlatform/atomicDEX-API/pull/1821)
- CI flows for `adex-cli` added [#1818](https://github.com/KomodoPlatform/atomicDEX-API/pull/1818)
- Detect a chain reorganization, if it occurs, redownload and revalidate the new best chain headers for SPV  [#1728](https://github.com/KomodoPlatform/atomicDEX-API/pull/1728)
- Fix moralis request in wasm target, add moralis tests [#1817](https://github.com/KomodoPlatform/atomicDEX-API/pull/1817)
- PoSV support for UTXO coins was added in [#1815](https://github.com/KomodoPlatform/atomicDEX-API/pull/1815)
- Use a new testnet for ETH tests, reduce the amount of ETH and ERC20 tokens exchanged, use fixed addresses instead of one-time use random addresses, fix some existing bugs (https://github.com/KomodoPlatform/atomicDEX-API/pull/1828)


## v1.0.3-beta - 2023-04-28

**Features:**

**Enhancements/Fixes:**
- cosmos/iris ethermint account compatibility implemented [#1765](https://github.com/KomodoPlatform/atomicDEX-API/pull/1765)
- p2p stack is improved [#1755](https://github.com/KomodoPlatform/atomicDEX-API/pull/1755)
  - Validate topics if they are mixed or not.
  - Do early return if the message data is not valid (since no point to iterate over and over on the invalid message)
  - Break the loop right after processing any of `SWAP_PREFIX`, `WATCHER_PREFIX`, `TX_HELPER_PREFIX` topic.
- An issue was fixed where we don't have to wait for all EVM nodes to sync the latest account nonce [#1757](https://github.com/KomodoPlatform/atomicDEX-API/pull/1757)
- optimized dev and release compilation profiles and removed ci [#1759](https://github.com/KomodoPlatform/atomicDEX-API/pull/1759)
- fix receiver trade fee for cosmos swaps [#1767](https://github.com/KomodoPlatform/atomicDEX-API/pull/1767)
- All features were enabled to be checked under x86-64 code lint CI step with `--all-features` option [#1760](https://github.com/KomodoPlatform/atomicDEX-API/pull/1760)
- use OS generated secrets for cryptographically secure randomness in `maker_swap` and `tendermint_coin::get_sender_trade_fee_for_denom` [#1753](https://github.com/KomodoPlatform/atomicDEX-API/pull/1753)


## v1.0.2-beta - 2023-04-11

**Features:**
- `adex-cli` command line utility was introduced that supplies commands: `init`, `start`, `stop`, `status` [#1729](https://github.com/KomodoPlatform/atomicDEX-API/pull/1729)

**Enhancements/Fixes:**
- CI/CD workflow logics are improved [#1736](https://github.com/KomodoPlatform/atomicDEX-API/pull/1736)
- Project root is simplified/refactored [#1738](https://github.com/KomodoPlatform/atomicDEX-API/pull/1738)
- Created base image to provide more glibc compatible pre-built binaries for linux [#1741](https://github.com/KomodoPlatform/atomicDEX-API/pull/1741)
- Set default log level as "info" [#1747](https://github.com/KomodoPlatform/atomicDEX-API/pull/1747)
- Refactor `native_log` module  [#1751](https://github.com/KomodoPlatform/atomicDEX-API/pull/1751)
  - implement stdout/err streaming to persistent file without dependencies
  - Add new parameter `silent_console` to mm conf


## v1.0.1-beta - 2023-03-17

**Features:**
- NFT integration `WIP` [#900](https://github.com/KomodoPlatform/atomicDEX-API/issues/900)
  - NFT integration PoC added. Includes ERC721 support for ETH and BSC [#1652](https://github.com/KomodoPlatform/atomicDEX-API/pull/1652)
  - Withdraw ERC1155 and EVM based chains support added for NFT PoC [#1704](https://github.com/KomodoPlatform/atomicDEX-API/pull/1704)
- Swap watcher nodes [#1431](https://github.com/KomodoPlatform/atomicDEX-API/issues/1431)
  - Watcher rewards for ETH swaps were added [#1658](https://github.com/KomodoPlatform/atomicDEX-API/pull/1658)
- Cosmos integration `WIP` [#1432](https://github.com/KomodoPlatform/atomicDEX-API/issues/1432)
  - `ibc_withdraw`, `ibc_chains` and `ibc_transfer_channels` RPC methods were added [#1636](https://github.com/KomodoPlatform/atomicDEX-API/pull/1636)
- Lightning integration `WIP` [#1045](https://github.com/KomodoPlatform/atomicDEX-API/issues/1045)
  - [rust-lightning](https://github.com/lightningdevkit/rust-lightning) was updated to [v0.0.113](https://github.com/lightningdevkit/rust-lightning/releases/tag/v0.0.113) in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655)
  - Channel `current_confirmations` and `required_confirmations` were added to channel details RPCs in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655)
  - `Uuid` is now used for internal channel id instead of `u64` [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655)
  - Some unit tests were added for multi path payments in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655)
  - Some unit tests for claiming swaps on-chain for closed channels were added in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655), these unit tests are currently failing.
  - `protocol_info` fields are now used to check if a lightning order can be matched or not in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655)
  - 2 issues discovered while executing a KMD/LNBTC swap were fixed in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655), these issues were:
    - When electrums were down, if a channel was closed, the channel closing transaction wasn't broadcasted. A check for a network error to rebroadcast the tx after a delay was added.
    - If an invoice payment failed, retring paying the same invoice would cause the payment to not be updated to successful in the DB even if it were successful. A method to update the payment in the DB was added to fix this.
  - `mm2_git` crate was added to provide an abstraction layer on Git for doing query/parse operations over the repositories [#1636](https://github.com/KomodoPlatform/atomicDEX-API/pull/1636)

**Enhancements/Fixes:**
- Use `env_logger` to achieve flexible log filtering [#1725](https://github.com/KomodoPlatform/atomicDEX-API/pull/1725)
- IndexedDB Cursor can now iterate over the items step-by-step [#1678](https://github.com/KomodoPlatform/atomicDEX-API/pull/1678)
- Uuid lib was update from v0.7.4 to v1.2.2 in [#1655](https://github.com/KomodoPlatform/atomicDEX-API/pull/1655)
- A bug was fixed in [#1706](https://github.com/KomodoPlatform/atomicDEX-API/pull/1706) where EVM swap transactions were failing if sent before the approval transaction confirmation.
- Tendermint account sequence problem due to running multiple instances were fixed in [#1694](https://github.com/KomodoPlatform/atomicDEX-API/pull/1694)
- Maker/taker pubkeys were added to new columns in `stats_swaps` table in [#1665](https://github.com/KomodoPlatform/atomicDEX-API/pull/1665) and [#1717](https://github.com/KomodoPlatform/atomicDEX-API/pull/1717)
- Get rid of unnecessary / old dependencies: `crossterm`, `crossterm_winapi`, `mio 0.7.13`, `miow`, `ntapi`, `signal-hook`, `signal-hook-mio` in [#1710](https://github.com/KomodoPlatform/atomicDEX-API/pull/1710)
- A bug that caused EVM swap payments validation to fail because the tx was not available yet in the RPC node when calling `eth_getTransactionByHash` was fixed in [#1716](https://github.com/KomodoPlatform/atomicDEX-API/pull/1716). `eth_getTransactionByHash` in now retried in `wait_for_confirmations` until tx is found in the RPC node, this makes sure that the transaction is returned from `eth_getTransactionByHash` later when validating.
- CI/CD migrated from Azure to Github runners [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- CI tests are much stabilized [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Integration and unit tests are seperated on CI stack [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Codebase is updated in linting rules at wasm and test stack [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- `crossbeam` bumped to `0.8` from `0.7` [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Un-used/Unstable parts of mm2 excluded from build outputs which avoids mm2 runtime from potential UB [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Build time optimizations applied such as sharing generics instead of duplicating them in binary (which reduces output sizes) [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- `RUSTSEC-2020-0036`, `RUSTSEC-2021-0139` and `RUSTSEC-2023-0018` resolved [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Enabled linting checks for wasm and test stack on CI [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Release container base image updated to debian stable from ubuntu bionic [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- Fix dylib linking error of rusb [#1699](https://github.com/KomodoPlatform/atomicDEX-API/pull/1699)
- `OperationFailure::Other` error was expanded. New error variants were matched with `HwRpcError`, so error type will be `HwError`, not `InternalError` [#1719](https://github.com/KomodoPlatform/atomicDEX-API/pull/1719)
- RPC calls for evm chains was reduced in `wait_for_confirmations` function in [#1724](https://github.com/KomodoPlatform/atomicDEX-API/pull/1724)
- A possible endless loop in evm `wait_for_htlc_tx_spend` was fixed in [#1724](https://github.com/KomodoPlatform/atomicDEX-API/pull/1724)
- Wait time for taker fee validation was increased from 30 to 60 seconds in [#1730](https://github.com/KomodoPlatform/atomicDEX-API/pull/1730) to give the fee tx more time to appear in most nodes mempools.

## v1.0.0-beta - 2023-03-08

**Features:**
- ARRR integration [#927](https://github.com/KomodoPlatform/atomicDEX-API/issues/927):
  - Zcoin native mode support was added [#1438](https://github.com/KomodoPlatform/atomicDEX-API/pull/1438)
  - Multi lightwalletd servers support was added [#1472](https://github.com/KomodoPlatform/atomicDEX-API/pull/1472)
  - Allow passing Zcash params file path to activation request [#1538](https://github.com/KomodoPlatform/atomicDEX-API/pull/1538)
  - Checksum verification of Zcash params files was added  [#1549](https://github.com/KomodoPlatform/atomicDEX-API/pull/1549)
- Tendermint integration [#1432](https://github.com/KomodoPlatform/atomicDEX-API/issues/1432)
  - Tendermint HTLC implementation [#1454](https://github.com/KomodoPlatform/atomicDEX-API/pull/1454)
  - Tendermint swap support (POC level) [#1468](https://github.com/KomodoPlatform/atomicDEX-API/pull/1454)
  - Complete tendermint support for swaps and tx history implementation [#1526](https://github.com/KomodoPlatform/atomicDEX-API/pull/1526)
  - Improve rpc client rotation of tendermint [#1675](https://github.com/KomodoPlatform/atomicDEX-API/pull/1675)
- HD Wallet [#740](https://github.com/KomodoPlatform/atomicDEX-API/issues/740)
  - Implement Global HD account activation mode [#1512](https://github.com/KomodoPlatform/atomicDEX-API/pull/1512)
  - `mm2_rmd160` property was removed from the HD account table. Now, either Iguana or an HD account share the same HD account records [#1672](https://github.com/KomodoPlatform/atomicDEX-API/pull/1672)
- Hardware Wallet [#964](https://github.com/KomodoPlatform/atomicDEX-API/issues/964)
  - Implement TX history V2 for UTXO coins activated with a Hardware wallet [#1467](https://github.com/KomodoPlatform/atomicDEX-API/pull/1467)
  - Fix KMD withdraw with Trezor [#1628](https://github.com/KomodoPlatform/atomicDEX-API/pull/1628)
  - `task::get_new_address::*` RPCs were added to replace the legacy `get_new_address` RPC [#1672](https://github.com/KomodoPlatform/atomicDEX-API/pull/1672)
  - `trezor_connection_status` RPC was added to allow the GUI to poll the Trezor connection status [#1672](https://github.com/KomodoPlatform/atomicDEX-API/pull/1672)
- Simple Payment Verification [#1612](https://github.com/KomodoPlatform/atomicDEX-API/issues/1612)
  - Implement unit test for `Block header UTXO Loop` [#1519](https://github.com/KomodoPlatform/atomicDEX-API/pull/1519)
  - `SPV` with minimal storage requirements and fast block headers sync time was implemented [#1585](https://github.com/KomodoPlatform/atomicDEX-API/pull/1585)
  - Block headers storage was implemented for `IndexedDB` [#1644](https://github.com/KomodoPlatform/atomicDEX-API/pull/1644)
  - `SPV` was re-enabled in `WASM` [#1644](https://github.com/KomodoPlatform/atomicDEX-API/pull/1644)
- New RPCs
  - gui-auth and `enable_eth_with_tokens` `enable_erc20` RPCs were added [#1335](https://github.com/KomodoPlatform/atomicDEX-API/pull/1335)
  - `get_current_mtp` RPC was added [#1340](https://github.com/KomodoPlatform/atomicDEX-API/pull/1340)
  - `max_maker_vol` RPC was added [#1618](https://github.com/KomodoPlatform/atomicDEX-API/pull/1618)
- Lightning integration `WIP` [#1045](https://github.com/KomodoPlatform/atomicDEX-API/issues/1045)
  - [rust-lightning](https://github.com/lightningdevkit/rust-lightning) was updated to [v0.0.110](https://github.com/lightningdevkit/rust-lightning/releases/tag/v0.0.110) in [#1452](https://github.com/KomodoPlatform/atomicDEX-API/pull/1452)
  - Inbound channels details was added to SQL channels history in [#1339](https://github.com/KomodoPlatform/atomicDEX-API/pull/1339)
  - Blocking was fixed for sync rust-lightning functions that calls other I/O functions or that has mutexes that can be held for some time in [#1452](https://github.com/KomodoPlatform/atomicDEX-API/pull/1452)
  - Default fees are retrieved from rpc instead of config when starting lightning [#1452](https://github.com/KomodoPlatform/atomicDEX-API/pull/1452)
  - 0 confirmations channels feature was added in [#1452](https://github.com/KomodoPlatform/atomicDEX-API/pull/1452)
  - An `update_channel` RPC was added that updates a channel that is open without closing it in [#1452](https://github.com/KomodoPlatform/atomicDEX-API/pull/1452)
  - Lightning RPCs now use the `lightning::` namespace in [#1497](https://github.com/KomodoPlatform/atomicDEX-API/pull/1497)
  - `TakerFee` and `MakerPayment` swap messages were modified to include payment instructions for the other side, in the case of lightning this payment instructions is a lightning invoice [#1497](https://github.com/KomodoPlatform/atomicDEX-API/pull/1497)
  - `MakerPaymentInstructionsReceived`/`TakerPaymentInstructionsReceived` events are added to `MakerSwapEvent`/`TakerSwapEvent` in [#1497](https://github.com/KomodoPlatform/atomicDEX-API/pull/1497), for more info check this [comment](https://github.com/KomodoPlatform/atomicDEX-API/issues/1045#issuecomment-1410449770)
  - Lightning swaps were implemented in [#1497](https://github.com/KomodoPlatform/atomicDEX-API/pull/1497), [#1557
    ](https://github.com/KomodoPlatform/atomicDEX-API/pull/1557)
  - Lightning swap refunds were implemented in [#1592](https://github.com/KomodoPlatform/atomicDEX-API/pull/1592)
  - `MakerPaymentRefundStarted`, `TakerPaymentRefundStarted`, `MakerPaymentRefundFinished`, `TakerPaymentRefundFinished` events were added to swap error events in [#1592](https://github.com/KomodoPlatform/atomicDEX-API/pull/1592), for more info check this [comment](https://github.com/KomodoPlatform/atomicDEX-API/issues/1045#issuecomment-1410449770)
  - Enabling lightning now uses the task manager [#1513](https://github.com/KomodoPlatform/atomicDEX-API/pull/1513)
  - Disabling lightning coin or calling `stop` RPC now drops the `BackgroundProcessor` which persists the latest network graph and scorer to disk [#1513](https://github.com/KomodoPlatform/atomicDEX-API/pull/1513), [#1490](https://github.com/KomodoPlatform/atomicDEX-API/pull/1490)
  - `avg_blocktime` from platform/utxo coin is used for l2/lightning estimating of the number of blocks swap payments are locked for [#1606](https://github.com/KomodoPlatform/atomicDEX-API/pull/1606)
- MetaMask `WIP` [#1167](https://github.com/KomodoPlatform/atomicDEX-API/issues/1167)
  - Login with a MetaMask wallet [#1551](https://github.com/KomodoPlatform/atomicDEX-API/pull/1551)
  - Check if corresponding ETH chain is known by MetaMask wallet on coin activation using `wallet_switchEthereumChain` [#1674](https://github.com/KomodoPlatform/atomicDEX-API/pull/1674)
  - Refactor ETH/ERC20 withdraw taking into account that the only way to sign a transaction is to send it using `eth_sendTransaction` [#1674](https://github.com/KomodoPlatform/atomicDEX-API/pull/1674)
  - Extract address's public key using `eth_singTypedDataV4` [#1674](https://github.com/KomodoPlatform/atomicDEX-API/pull/1674)
  - Perform swaps with coins activated with MetaMask [#1674](https://github.com/KomodoPlatform/atomicDEX-API/pull/1674)
   
**Enhancements/Fixes:**
- Update `rust-web3` crate [#1674](https://github.com/KomodoPlatform/atomicDEX-API/pull/1674)
- Custom enum from stringify derive macro to derive From implementations for enums  [#1502](https://github.com/KomodoPlatform/atomicDEX-API/pull/1502)
- Validate that  `input_tx` is calling `'receiverSpend'` in `eth::extract_secret` [#1596](https://github.com/KomodoPlatform/atomicDEX-API/pull/1596)
- Validate all Swap parameters at the Negotiation stage [#1475](https://github.com/KomodoPlatform/atomicDEX-API/pull/1475)
- created safe number type castings [#1517](https://github.com/KomodoPlatform/atomicDEX-API/pull/1517)
- Improve `stop` functionality [#1490](https://github.com/KomodoPlatform/atomicDEX-API/pull/1490)
- A possible seednode p2p thread panicking attack due to `GetKnownPeers` msg was fixed in [#1445](https://github.com/KomodoPlatform/atomicDEX-API/pull/1445)
- NAV `cold_staking` script type was added to fix a problem in NAV tx history in [#1466](https://github.com/KomodoPlatform/atomicDEX-API/pull/1466)
- SPV was temporarily disabled in WASM in [#1479](https://github.com/KomodoPlatform/atomicDEX-API/pull/1479)
- `BTC-segwit` swap locktimes was fixed in [#1548](https://github.com/KomodoPlatform/atomicDEX-API/pull/1548) by using orderbook ticker instead of ticker in swap locktimes calculations.
- BTC block headers deserialization was fixed for version 4 and `KAWPOW_VERSION` in [#1452](https://github.com/KomodoPlatform/atomicDEX-API/pull/1452)
- Error messages for failing swaps due to a time difference between maker and taker are now more informative after [#1677](https://github.com/KomodoPlatform/atomicDEX-API/pull/1677)
- Fix `LBC` block header deserialization bug [#1343](https://github.com/KomodoPlatform/atomicDEX-API/pull/1343)
- Fix `NMC` block header deserialization bug [#1409](https://github.com/KomodoPlatform/atomicDEX-API/pull/1409)
- Refactor mm2 error handling for some structures [#1444](https://github.com/KomodoPlatform/atomicDEX-API/pull/1444)
- Tx wait for confirmation timeout fix [#1446](https://github.com/KomodoPlatform/atomicDEX-API/pull/1446)
- Retry tx wait confirmation if not on chain [#1474](https://github.com/KomodoPlatform/atomicDEX-API/pull/1474)
- Fix electrum "response is too large (over 2M bytes)" error for block header download [#1506](https://github.com/KomodoPlatform/atomicDEX-API/pull/1506)
- Deactivate tokens with platform coin [#1525](https://github.com/KomodoPlatform/atomicDEX-API/pull/1525)
- Enhanced logging in` spv` and `rpc_client` mods [#1594](https://github.com/KomodoPlatform/atomicDEX-API/pull/1594)
- Update metrics related dep && refactoring [#1312](https://github.com/KomodoPlatform/atomicDEX-API/pull/1312)
- Fix rick and morty genesis block deserialization [#1647](https://github.com/KomodoPlatform/atomicDEX-API/pull/1647)
- In `librustzcash` bumped `bech32` to `0.9.1`(which we already have in mm2, so we will not have 2 versions of `bech32`)
- Use dev branch as a target branch for Dependabot [#1424](https://github.com/KomodoPlatform/atomicDEX-API/pull/1424)
- Fixed Zhtlc orders is_mine bug (orders had "is_mine":false)  [#1489](https://github.com/KomodoPlatform/atomicDEX-API/pull/1489)
- Grouped SwapOps method arguments into new groups(structures) [#1529](https://github.com/KomodoPlatform/atomicDEX-API/pull/1529)
- Handling multiple rpcs optimization [#1480](https://github.com/KomodoPlatform/atomicDEX-API/issues/1480)
  - Tendermint multiple rpcs optimization [#1568](https://github.com/KomodoPlatform/atomicDEX-API/pull/1568)
  - Multiple rpcs optimization for `z_rpc` and `http_transport` [#1653](https://github.com/KomodoPlatform/atomicDEX-API/pull/1653)
  - Refactor p2p message processing flow (related with one of the security problem) [#1436](https://github.com/KomodoPlatform/atomicDEX-API/pull/1436)
- Solana tests are disabled [#1660](https://github.com/KomodoPlatform/atomicDEX-API/pull/1660)
- Some of vulnerable dependencies(tokio, libp2p) are fixed [#1666](https://github.com/KomodoPlatform/atomicDEX-API/pull/1666)
- Add `mm2_stop` WASM FFI [#1628](https://github.com/KomodoPlatform/atomicDEX-API/pull/1628)
- Use `futures_timer` crate and fix some unstable tests [#1511](https://github.com/KomodoPlatform/atomicDEX-API/pull/1511)
- Fix `Timer::sleep_ms` in WASM [#1514](https://github.com/KomodoPlatform/atomicDEX-API/pull/1514)
- Fix a race condition in `AbortableQueue` [#1528](https://github.com/KomodoPlatform/atomicDEX-API/pull/1528)
- Spawn `process_json_request` so the RPC requests can be processed asynchronously [#1620](https://github.com/KomodoPlatform/atomicDEX-API/pull/1620)
- Fix `task::-::cancel` if the RPC task is an awaiting status [#1582](https://github.com/KomodoPlatform/atomicDEX-API/pull/1582)
- `disable_coin` should fail if there are tokens dependent on the platform [#1651](https://github.com/KomodoPlatform/atomicDEX-API/pull/1651)
- Implement a repeatable future [#1564](https://github.com/KomodoPlatform/atomicDEX-API/pull/1564)
- Version handling was enhanced [#1686](https://github.com/KomodoPlatform/atomicDEX-API/pull/1686)
  - Version of `mm2_bin_lib` from cargo manifest is now used for the API version
  - `--version`, `-v`, `version` arguments now print the mm2 version
- Workflow for VirusTotal results was added to CI [#1676](https://github.com/KomodoPlatform/atomicDEX-API/pull/1676)
- `parity-ethereum` and `testcontainers-rs` crates from KomodoPlatform repo are now used [#1690](https://github.com/KomodoPlatform/atomicDEX-API/pull/1690)
- Testnet node of atom was updated, RUSTSEC-2023-0018 was ignored [#1692](https://github.com/KomodoPlatform/atomicDEX-API/pull/1692)
- Timestamp value sent from the peer in `PubkeyKeepAlive` msg was ignored and the received timestamp was used instead [#1668](https://github.com/KomodoPlatform/atomicDEX-API/pull/1668)
- Change release branch from mm2.1 to main in CI [#1697](https://github.com/KomodoPlatform/atomicDEX-API/pull/1697)
- CHANGELOG.md was introduced to have a complete log of code changes [#1680](https://github.com/KomodoPlatform/atomicDEX-API/pull/1680)
- Small fixes [#1518](https://github.com/KomodoPlatform/atomicDEX-API/pull/1518), [#1515](https://github.com/KomodoPlatform/atomicDEX-API/pull/1515), [#1550](https://github.com/KomodoPlatform/atomicDEX-API/pull/1657), [#1657](https://github.com/KomodoPlatform/atomicDEX-API/pull/1657)

**NB - Backwards compatibility breaking changes:**
- Because of [#1548](https://github.com/KomodoPlatform/atomicDEX-API/pull/1548), old nodes will not be able to swap BTC segwit with new nodes since locktimes are exchanged and validated in the negotiation messages.
