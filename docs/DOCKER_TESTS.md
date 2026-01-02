# Docker Tests

Docker tests run against local blockchain nodes to verify atomic swap functionality.

## Prerequisites

1. **Docker**: Install Docker Desktop or Docker Engine
2. **Zcash Parameters** (for UTXO nodes):
   ```bash
   wget -O - https://raw.githubusercontent.com/KomodoPlatform/komodo/v0.8.1/zcutil/fetch-params-alt.sh | bash
   ```

## Quick Start

```bash
# Run all tests (testcontainers mode - starts containers automatically)
cargo test --test docker_tests_main --features docker-tests-all
```

## Running Specific Test Suites

Tests are split by feature flag. Use the flag for the suite you want:

| Feature | What it tests |
|---------|---------------|
| `docker-tests-eth` | ETH/ERC20/NFT |
| `docker-tests-slp` | BCH/SLP tokens |
| `docker-tests-sia` | Sia |
| `docker-tests-qrc20` | Qtum/QRC20 |
| `docker-tests-tendermint` | Cosmos/IBC |
| `docker-tests-zcoin` | ZCoin/Zombie |
| `docker-tests-swaps` | Swap protocol |
| `docker-tests-ordermatch` | Ordermatching |
| `docker-tests-watchers` | Watcher nodes |
| `docker-tests-integration` | Cross-chain swaps |
| `docker-tests-all` | Everything |

```bash
# Example: run only ETH tests
cargo test --test docker_tests_main --features docker-tests-eth
```

## Docker Compose Mode (Faster Development)

Keep nodes running between test runs for faster iteration:

```bash
# 1. Prepare environment (needed for Cosmos & Sia tests)
./scripts/ci/docker-test-nodes-setup.sh

# 2. Start nodes (use profile for specific chains)
docker compose -f .docker/test-nodes.yml --profile all up -d

# 3. Run tests against running containers
KDF_DOCKER_COMPOSE_ENV=1 cargo test --test docker_tests_main --features docker-tests-eth

# 4. Stop when done
docker compose -f .docker/test-nodes.yml --profile all down -v
```

**Profiles**: `utxo`, `slp`, `qrc20`, `evm`, `zombie`, `cosmos`, `sia`, `all`

## Troubleshooting

**Containers won't start**: Check Docker is running (`docker info`)

**Port conflicts**: Stop existing containers (`docker compose -f .docker/test-nodes.yml down`)

**Stale state**: Clean up and restart:
```bash
docker compose -f .docker/test-nodes.yml down -v
rm -rf .docker/container-runtime
./scripts/ci/docker-test-nodes-setup.sh
```

**UTXO nodes fail**: Ensure zcash params are downloaded (see Prerequisites)

## Test Nodes

| Node | Image | Port |
|------|-------|------|
| MYCOIN/MYCOIN1 | `gleec/testblockchain:multiarch` | 8000/8001 |
| FORSLP | `gleec/testblockchain:multiarch` | 10000 |
| QTUM | `gleec/qtumregtest:latest` | 9000 |
| GETH | `ethereum/client-go:stable` | 8545 |
| ZOMBIE | `gleec/zombietestrunner:multiarch` | 7090 |
| NUCLEUS/ATOM | `gleec/nucleusd:latest`, `gleec/gaiad:kdf-ci` | 26657/26658 |
| SIA | `ghcr.io/siafoundation/walletd:latest` | 9980 |
