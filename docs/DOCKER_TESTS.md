# Docker Tests Infrastructure

This document describes the docker-based test infrastructure for KDF (Komodo DeFi Framework).

## Overview

KDF docker tests run against local blockchain test nodes to verify atomic swap functionality, coin implementations, and integration scenarios. The infrastructure supports 10 different blockchain nodes:

| Node | Image | Port | Purpose |
|------|-------|------|---------|
| MYCOIN | `artempikulin/testblockchain:multiarch` | 8000 | UTXO testing |
| MYCOIN1 | `artempikulin/testblockchain:multiarch` | 8001 | UTXO testing (second node) |
| FORSLP | `artempikulin/testblockchain:multiarch` | 10000 | BCH/SLP token testing |
| QTUM | `sergeyboyko/qtumregtest:latest` | 9000 | Qtum/QRC20 testing |
| GETH | `ethereum/client-go:stable` | 8545 | Ethereum/ERC20/NFT testing |
| ZOMBIE | `borngraced/zombietestrunner:multiarch` | 7090 | Zcash-based testing |
| NUCLEUS | `komodoofficial/nucleusd:latest` | 26657 | Tendermint testing |
| ATOM | `komodoofficial/gaiad:kdf-ci` | 26658 | Cosmos testing |
| IBC-RELAYER | `komodoofficial/ibc-relayer:kdf-ci` | - | IBC channel relay |
| SIA | `siafoundation/walletd:latest` | 9980 | Sia testing |

## Running Docker Tests

### Prerequisites

1. **Docker**: Install Docker Desktop or Docker Engine
2. **Zcash Parameters**: Required for UTXO nodes
   ```bash
   wget -O - https://raw.githubusercontent.com/KomodoPlatform/komodo/v0.8.1/zcutil/fetch-params-alt.sh | bash
   ```

### Quick Start (Current Method)

The test harness automatically manages containers using testcontainers:

```bash
cargo test --test 'docker_tests_main' --features run-docker-tests
```

### Using Docker Compose (Recommended for Development)

For faster iteration during development, use docker-compose to keep nodes running:

```bash
# 1. Prepare the runtime environment
./scripts/ci/docker-test-nodes-setup.sh

# 2. Start all test nodes
docker compose -f .docker/test-nodes.yml --profile all up -d

# 3. Run tests with external nodes
KDF_DOCKER_COMPOSE_ENV=1 cargo test --test 'docker_tests_main' --features run-docker-tests

# 4. Run additional test suites (reuses same nodes)
KDF_DOCKER_ENV_STATE_FILE=.docker/container-runtime/docker_env_state.json \
  cargo test --test 'docker_tests_main' --features run-docker-tests -- specific_test

# 5. Stop nodes when done
docker compose -f .docker/test-nodes.yml down -v
```

### Selective Node Startup

Use profiles to start only needed nodes:

```bash
# UTXO tests only
docker compose -f .docker/test-nodes.yml --profile utxo up -d

# EVM tests only
docker compose -f .docker/test-nodes.yml --profile evm up -d

# Multiple profiles
docker compose -f .docker/test-nodes.yml --profile utxo --profile evm up -d
```

Available profiles:
- `utxo` - MYCOIN, MYCOIN1
- `slp` - FORSLP
- `qrc20` - QTUM
- `evm` - GETH
- `zombie` - ZOMBIE
- `cosmos` - NUCLEUS, ATOM, IBC-RELAYER
- `sia` - SIA
- `all` - All nodes

### Skipping Specific Nodes

Use environment variables to skip node groups:

```bash
# Skip Ethereum tests
_KDF_NO_ETH_DOCKER=1 cargo test --test 'docker_tests_main' --features run-docker-tests

# Skip Cosmos tests
_KDF_NO_COSMOS_DOCKER=1 cargo test --test 'docker_tests_main' --features run-docker-tests

# Skip multiple
_KDF_NO_ETH_DOCKER=1 _KDF_NO_COSMOS_DOCKER=1 cargo test --test 'docker_tests_main' --features run-docker-tests
```

Available skip variables:
- `_KDF_NO_UTXO_DOCKER` - Skip MYCOIN/MYCOIN1
- `_KDF_NO_SLP_DOCKER` - Skip FORSLP
- `_KDF_NO_QTUM_DOCKER` - Skip QTUM
- `_KDF_NO_ETH_DOCKER` - Skip GETH
- `_KDF_NO_ZOMBIE_DOCKER` - Skip ZOMBIE
- `_KDF_NO_COSMOS_DOCKER` - Skip NUCLEUS/ATOM/IBC-RELAYER
- `_KDF_NO_SIA_DOCKER` - Skip SIA

## Environment Variables

| Variable | Description |
|----------|-------------|
| `KDF_DOCKER_COMPOSE_ENV` | When set to `1`, test harness attaches to running compose containers instead of starting new ones |
| `KDF_DOCKER_ENV_STATE_FILE` | Path to metadata JSON file; skips both container start and initialization |
| `KDF_CONTAINER_RUNTIME_DIR` | Override path to container runtime data (default: `.docker/container-runtime`) |
| `ZCASH_PARAMS_PATH` | Path to zcash-params directory (default: `~/.zcash-params`) |

## Architecture

### Container Management

The test infrastructure has two modes:

1. **Testcontainers Mode** (default): Each test run starts fresh containers that are automatically cleaned up. Uses the `testcontainers` Rust crate.

2. **Docker Compose Mode** (development): Containers run independently, allowing multiple test runs to share the same initialized nodes.

### Initialization Flow

When nodes start, the test harness performs initialization:

1. **UTXO Nodes**: Wait for RPC readiness
2. **Qtum**: Deploy QRC20 token and swap contracts
3. **BCH/SLP**: Mint SLP tokens and distribute to test wallets
4. **Geth**: Deploy ERC20, swap, NFT, and V2 contracts; fund test accounts
5. **Cosmos**: Wait for IBC relayer to establish channels
6. **Sia**: Mine initial blocks and start background miner

### State Persistence

When using `KDF_DOCKER_COMPOSE_ENV=1`, the harness writes initialization results to `.docker/container-runtime/docker_env_state.json`. This includes:

- Deployed contract addresses
- Minted token IDs
- Funded wallet keys
- RPC endpoints

Subsequent runs with `KDF_DOCKER_ENV_STATE_FILE` load this metadata instead of re-initializing.

## File Structure

```
.docker/
├── test-nodes.yml           # Docker Compose definition
├── container-state/         # Static config templates (committed)
│   ├── atom-testnet-data/
│   ├── nucleus-testnet-data/
│   └── ibc-relayer-data/
└── container-runtime/       # Runtime data (gitignored)
    ├── atom-testnet-data/
    ├── nucleus-testnet-data/
    ├── ibc-relayer-data/
    ├── sia-config/
    └── docker_env_state.json

scripts/ci/
└── docker-test-nodes-setup.sh  # Prepares runtime environment

mm2src/mm2_main/tests/
├── docker_tests_main.rs        # Test entry point
├── docker_tests/
│   ├── docker_tests_common.rs  # Node helpers and initialization
│   ├── qrc20_tests.rs          # Qtum-specific tests
│   ├── eth_docker_tests.rs     # Ethereum tests
│   ├── slp_tests.rs            # SLP token tests
│   └── ...
└── sia_tests/
    └── utils.rs                # Sia test utilities
```

## Troubleshooting

### Containers not starting

Check Docker is running:
```bash
docker info
```

View container logs:
```bash
docker compose -f .docker/test-nodes.yml logs -f <service>
```

### Port conflicts

If ports are already in use:
```bash
# Check what's using a port
lsof -i :8545

# Stop all KDF test containers
docker compose -f .docker/test-nodes.yml down
```

### Stale state

If tests fail due to stale initialization:
```bash
# Clean up and restart
docker compose -f .docker/test-nodes.yml down -v
rm -rf .docker/container-runtime
./scripts/ci/docker-test-nodes-setup.sh
docker compose -f .docker/test-nodes.yml --profile all up -d
```

### Zcash params missing

If UTXO nodes fail to start:
```bash
wget -O - https://raw.githubusercontent.com/KomodoPlatform/komodo/v0.8.1/zcutil/fetch-params-alt.sh | bash
```

## CI Integration

### Split Test Jobs

The CI runs docker tests in separate jobs to improve parallelism and reduce resource usage:

| Job | Feature Flag | Compose Profile | Timeout | Tests |
|-----|--------------|-----------------|---------|-------|
| `docker-tests-slp` | `docker-tests-slp` | `slp` | 45 min | SLP/BCH token tests |
| `docker-tests-sia` | `docker-tests-sia` | `sia` | 30 min | Sia blockchain tests |
| `docker-tests-eth` | `docker-tests-eth` | `evm` | 60 min | ETH/ERC20/NFT tests |
| `docker-tests` | `run-docker-tests` | `all` | 90 min | All remaining tests |

Each chain-specific job:
- Starts only the required node(s) via compose profile
- Sets `_KDF_NO_*` env vars to disable other node groups
- Uses the corresponding feature flag for compilation
- Filters tests with `-- <module>::` pattern

Cross-chain swap tests (`swap_tests`) only run in the main `docker-tests` job since they require multiple node types.

### Main docker-tests Job

The GitHub Actions workflow (`.github/workflows/test.yml`) runs docker tests in the `docker-tests` job:

1. Checks out code
2. Installs Rust toolchain
3. Fetches zcash-params
4. Prepares runtime environment (`./scripts/ci/docker-test-nodes-setup.sh`)
5. Starts all test nodes via docker-compose (`--profile all`)
6. Runs tests with `KDF_DOCKER_COMPOSE_ENV=1` (attaches to compose containers)
7. Stops containers (`docker compose down -v`) - runs even if tests fail

The workflow uses docker-compose mode rather than testcontainers, which enables:
- Faster startup (containers already running when tests start)
- Better visibility into container state during debugging
- Future ability to run multiple test binaries against the same nodes

## Execution Modes

The test harness supports three execution modes:

| Mode | Trigger | Container Start | Initialization |
|------|---------|-----------------|----------------|
| **Testcontainers** | Default (no env vars) | ✅ Via testcontainers | ✅ Full |
| **ComposeInit** | `KDF_DOCKER_COMPOSE_ENV=1` | ❌ Assumes running | ✅ Full (saves metadata) |
| **ReuseMetadata** | `KDF_DOCKER_ENV_STATE_FILE=path` | ❌ Assumes running | ❌ Loads from file |

### Mode Selection Logic

```
if KDF_DOCKER_ENV_STATE_FILE is set:
    → ReuseMetadata mode
    → Load metadata, validate node health, skip initialization
elif KDF_DOCKER_COMPOSE_ENV is set:
    → ComposeInit mode
    → Attach to running containers, run initialization, save metadata
else:
    → Testcontainers mode
    → Start fresh containers, run initialization
```

### Health Checks

When loading metadata in ReuseMetadata mode, the harness validates that all initialized nodes are reachable before proceeding. If any health check fails, tests abort with an error message indicating which node is unreachable.

## Future Refactoring

### Modularizing docker_tests_common.rs

The current `docker_tests_common.rs` file contains helpers for all blockchain types mixed together. This makes it difficult to use feature flags to compile only the tests needed for a specific chain.

**Current state:**
- ETH, UTXO, SLP, Cosmos, Sia, and other helpers are in one file
- Functions reference types from multiple chain implementations
- Feature-flag based test isolation requires scattered `#[cfg(...)]` annotations

**Planned refactoring:**
1. Split `docker_tests_common.rs` into chain-specific modules:
   ```
   docker_tests/
   ├── helpers/
   │   ├── mod.rs          # Truly shared utilities (mm2 setup, test framework)
   │   ├── utxo.rs         # UTXO-specific helpers
   │   ├── eth.rs          # ETH/ERC20 helpers
   │   ├── slp.rs          # SLP token helpers
   │   ├── cosmos.rs       # Tendermint/IBC helpers
   │   ├── sia.rs          # Sia helpers
   │   └── zcoin.rs        # Z-coin helpers
   ```

2. Add feature flags for each chain type:
   ```toml
   # Cargo.toml
   docker-tests-slp = ["run-docker-tests"]
   docker-tests-eth = ["run-docker-tests"]
   docker-tests-utxo = ["run-docker-tests"]
   docker-tests-cosmos = ["run-docker-tests"]
   docker-tests-sia = ["run-docker-tests"]
   docker-tests-zcoin = ["run-docker-tests"]
   ```

3. Apply feature flags at module level:
   ```rust
   // helpers/mod.rs
   #[cfg(feature = "docker-tests-eth")]
   pub mod eth;

   #[cfg(feature = "docker-tests-slp")]
   pub mod slp;

   #[cfg(feature = "docker-tests-utxo")]
   pub mod utxo;
   // etc.
   ```

4. Each chain module would only depend on relevant imports

**Benefits:**
- Clean feature-flag isolation without scattered cfg annotations
- Faster compilation for targeted test runs
- Easier maintenance and testing of individual chains
- Better separation of concerns
- CI jobs can run in parallel with minimal resource usage per job
