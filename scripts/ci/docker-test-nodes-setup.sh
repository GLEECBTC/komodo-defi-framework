#!/bin/bash
#
# Setup script for KDF docker test nodes
#
# This script prepares the container runtime directory and configuration files
# needed by the docker-compose test environment.
#
# Usage:
#   ./scripts/ci/docker-test-nodes-setup.sh [--skip-cosmos] [--skip-sia]
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

CONTAINER_STATE_DIR="$PROJECT_ROOT/.docker/container-state"
CONTAINER_RUNTIME_DIR="$PROJECT_ROOT/.docker/container-runtime"

SKIP_COSMOS=false
SKIP_SIA=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-cosmos)
            SKIP_COSMOS=true
            shift
            ;;
        --skip-sia)
            SKIP_SIA=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "=== KDF Docker Test Nodes Setup ==="
echo "Project root: $PROJECT_ROOT"

# ============================================================================
# Prepare runtime directory for Cosmos nodes
# ============================================================================

if [ "$SKIP_COSMOS" = false ]; then
    echo ""
    echo "Preparing Cosmos node runtime directories..."

    if [ ! -d "$CONTAINER_STATE_DIR" ]; then
        echo "ERROR: Container state directory not found: $CONTAINER_STATE_DIR"
        exit 1
    fi

    # Remove existing runtime directory to start fresh
    if [ -d "$CONTAINER_RUNTIME_DIR" ]; then
        echo "Removing existing runtime directory..."
        rm -rf "$CONTAINER_RUNTIME_DIR"
    fi

    # Copy container state to runtime directory
    echo "Copying container state to runtime directory..."
    cp -r "$CONTAINER_STATE_DIR" "$CONTAINER_RUNTIME_DIR"

    # Set proper permissions
    chmod -R 755 "$CONTAINER_RUNTIME_DIR"

    echo "Cosmos node data prepared at: $CONTAINER_RUNTIME_DIR"
else
    echo "Skipping Cosmos node setup (--skip-cosmos)"
fi

# ============================================================================
# Prepare Sia configuration
# ============================================================================

if [ "$SKIP_SIA" = false ]; then
    echo ""
    echo "Preparing Sia node configuration..."

    SIA_CONFIG_DIR="$CONTAINER_RUNTIME_DIR/sia-config"
    mkdir -p "$SIA_CONFIG_DIR"

    # Write walletd.yml
    cat > "$SIA_CONFIG_DIR/walletd.yml" << 'EOF'
http:
  address: :9980
  password: password
  publicEndpoints: false
index:
  mode: full
log:
  stdout:
    enabled: true
    level: debug
    format: human
EOF

    # Write ci_network.json
    cat > "$SIA_CONFIG_DIR/ci_network.json" << 'EOF'
{
    "network": {
        "name": "komodo-ci",
        "initialCoinbase": "300000000000000000000000000000",
        "minimumCoinbase": "30000000000000000000000000000",
        "initialTarget": "0100000000000000000000000000000000000000000000000000000000000000",
        "blockInterval": 60000000000,
        "maturityDelay": 10,
        "hardforkDevAddr": {
            "height": 1,
            "oldAddress": "000000000000000000000000000000000000000000000000000000000000000089eb0d6a8a69",
            "newAddress": "000000000000000000000000000000000000000000000000000000000000000089eb0d6a8a69"
        },
        "hardforkTax": {
            "height": 2
        },
        "hardforkStorageProof": {
            "height": 5
        },
        "hardforkOak": {
            "height": 10,
            "fixHeight": 12,
            "genesisTimestamp": "2023-01-13T00:53:20-08:00"
        },
        "hardforkASIC": {
            "height": 20,
            "oakTime": 600000000000,
            "oakTarget": "0100000000000000000000000000000000000000000000000000000000000000",
            "nonceFactor": 1009
        },
        "hardforkFoundation": {
            "height": 30,
            "primaryAddress": "053b2def3cbdd078c19d62ce2b4f0b1a3c5e0ffbeeff01280efb1f8969b2f5bb4fdc680f0807",
            "failsafeAddress": "000000000000000000000000000000000000000000000000000000000000000089eb0d6a8a69"
        },
        "hardforkV2": {
            "allowHeight": 0,
            "requireHeight": 7777777,
            "finalCutHeight": 8888888
        }
    },
    "genesis": {
        "parentID": "0000000000000000000000000000000000000000000000000000000000000000",
        "nonce": 0,
        "timestamp": "2023-01-13T00:53:20-08:00",
        "minerPayouts": null,
        "transactions": [
            {
                "id": "268ef8627241b3eb505cea69b21379c4b91c21dfc4b3f3f58c66316249058cfd",
                "siacoinOutputs": [
                    {
                        "value": "1000000000000000000000000000000000000",
                        "address": "a0cfbc1089d129f52d00bc0b0fac190d4d87976a1d7f34da7ca0c295c99a628de344d19ad469"
                    }
                ],
                "siafundOutputs": [
                    {
                        "value": 10000,
                        "address": "053b2def3cbdd078c19d62ce2b4f0b1a3c5e0ffbeeff01280efb1f8969b2f5bb4fdc680f0807"
                    }
                ]
            }
        ]
    }
}
EOF

    echo "Sia configuration written to: $SIA_CONFIG_DIR"
else
    echo "Skipping Sia setup (--skip-sia)"
fi

# ============================================================================
# Export environment variables for docker-compose
# ============================================================================

echo ""
echo "=== Environment Variables ==="
echo "Set these environment variables before running docker-compose:"
echo ""
echo "  export KDF_CONTAINER_RUNTIME_DIR=$CONTAINER_RUNTIME_DIR"
echo "  export ZCASH_PARAMS_PATH=\${HOME}/.zcash-params"
echo ""
echo "Or use the defaults in the compose file."

# ============================================================================
# Summary
# ============================================================================

echo ""
echo "=== Setup Complete ==="
echo ""
echo "To start the test nodes:"
echo "  docker compose -f .docker/test-nodes.yml --profile all up -d"
echo ""
echo "To start specific profiles:"
echo "  docker compose -f .docker/test-nodes.yml --profile utxo --profile evm up -d"
echo ""
echo "To view logs:"
echo "  docker compose -f .docker/test-nodes.yml logs -f"
echo ""
echo "To stop and cleanup:"
echo "  docker compose -f .docker/test-nodes.yml down -v"
