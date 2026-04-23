# EtomicSwap V1 Deployment Guide

> **Related**: See [safeerc20-v1-usdt-support.md](./safeerc20-v1-usdt-support.md) for the SafeERC20 implementation plan.

This guide covers deploying EtomicSwap V1 with:
- **Hardhat Ignition** for declarative, reproducible deployments
- **CREATE2** for deterministic cross-chain addresses
- **Vanity address mining** to achieve `0x61eec...d3f1` (GLEEC...DEFI)

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Hardhat Ignition Setup](#2-hardhat-ignition-setup)
3. [CREATE2 Configuration](#3-create2-configuration)
4. [Vanity Address Mining](#4-vanity-address-mining)
5. [Deployment Process](#5-deployment-process)
6. [Verification](#6-verification)
7. [Security Considerations](#7-security-considerations)

---

## 1. Prerequisites

### Required Tools
- Node.js 18+
- Yarn package manager
- Hardhat 2.22.18+
- Deployer wallet with ETH on target networks

### Target Networks

> **Initial Deployment**: Ethereum Mainnet only. Other networks ready for future deployment.

#### Primary Target (Phase 1)
| Network | Chain ID | Coins Platform | ERC20 Tokens |
|---------|----------|----------------|--------------|
| Ethereum Mainnet | 1 | ETH | 169 |

#### Future Networks (Phase 2+)
All networks supported in the [coins repo](https://github.com/AtoMIC-DEX/coins):

**Mainnets (17 total):**
| Network | Chain ID | Coins Platform | ERC20 Tokens | Current Swap Contract |
|---------|----------|----------------|--------------|----------------------|
| Ethereum | 1 | ETH | 169 | `0x24ABE4c71FC658C91313b6552cd40cD808b3Ea80` |
| Binance Smart Chain | 56 | BNB | 172 | `0xeDc5b89Fe1f0382F9E4316069971D90a0951DB31` |
| Polygon | 137 | MATIC | 120 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |
| Avalanche C-Chain | 43114 | AVAX | 39 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |
| Arbitrum One | 42161 | ETH-ARB20 | 20 | `0x9130b257d37a52e52f21054c4da3450c72f595ce` |
| KuCoin Chain | 321 | KCS | 29 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |
| Moonriver | 1285 | MOVR | 7 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |
| Base | 8453 | ETH-BASE | 6 | `0x4C54808911817402a0871c3C93D8790c19CAe75b` |
| Fantom | 250 | FTM | 0 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |
| Moonbeam | 1284 | GLMR | 0 | `0x6d9ce4BD298DE38bAfEFD15f5C6f5c95313B1d94` |
| Harmony One | 1666600000 | ONE | 0 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |
| Ethereum Classic | 61 | ETC | 0 | `0x6D9CE4bD298de38Bafefd15F5C6F5c95313B1d94` |
| RSK (Rootstock) | 30 | RSK | 0 | `0x6D9CE4bD298de38Bafefd15F5C6F5c95313B1d94` |
| Energy Web Chain | 246 | EWT | 0 | `0x304896fc2F242f13dd852b412E7B60C5F495B79c` |
| SmartBCH | 10000 | SBCH | 0 | `0x25bF2AAB8749AD2e4360b3e0B738f3Cd700C4D68` |
| Qtum (EVM) | 3888 | QTUM | 0 | `0x2f754733acd6d753731c00fee32cb484551cc15d` |
| Ubiq | 8 | UBQ | 0 | `0x9130b257D37A52E52F21054c4DA3450c72f595CE` |

**Testnets (5 total):**
| Network | Chain ID | Coins Platform |
|---------|----------|----------------|
| Sepolia | 11155111 | - |
| BSC Testnet | 97 | BNBT |
| Polygon Mumbai | 80001 | MATICTEST |
| Avalanche Testnet | 43113 | AVAXT |
| Fantom Testnet | 4002 | FTMT |

> **Note on CREATE2**: CreateX factory (`0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed`) must be verified on each network before deployment. Check [CreateX deployments](https://github.com/pcaversaccio/createx) for supported networks.

---

## 2. Hardhat Ignition Setup

### 2.1 Install Dependencies

```bash
cd etomic-swap

yarn add --dev \
  @nomicfoundation/hardhat-ignition-ethers@hh2 \
  @nomicfoundation/hardhat-ignition@hh2 \
  @nomicfoundation/ignition-core@hh2 \
  @nomicfoundation/hardhat-verify@hh2 \
  dotenv
```

> **Important**: Use `@hh2` tagged versions for Hardhat 2.x compatibility.

### 2.2 Create Ignition Module

Create `ignition/modules/EtomicSwapV1.js`:

```javascript
const { buildModule } = require("@nomicfoundation/hardhat-ignition/modules");

module.exports = buildModule("EtomicSwapV1", (m) => {
  // Deploy contracts/EtomicSwap.sol (no constructor args)
  const etomicSwap = m.contract("EtomicSwap");

  return { etomicSwap };
});
```

### 2.3 Update hardhat.config.js

```javascript
require("@nomicfoundation/hardhat-ethers");
require("@nomicfoundation/hardhat-ignition-ethers");
require("@nomicfoundation/hardhat-verify");
require("dotenv").config();

const DEPLOYER_PRIVATE_KEY = process.env.DEPLOYER_PRIVATE_KEY;
const ETOMIC_SWAP_V1_CREATE2_SALT = process.env.ETOMIC_SWAP_V1_CREATE2_SALT;

if (!ETOMIC_SWAP_V1_CREATE2_SALT) {
  console.warn("Warning: Missing ETOMIC_SWAP_V1_CREATE2_SALT (required for CREATE2 deployment)");
}

// Helper to create network config
const networkConfig = (chainId, rpcEnvVar) => ({
  url: process.env[rpcEnvVar] || "",
  accounts: DEPLOYER_PRIVATE_KEY ? [DEPLOYER_PRIVATE_KEY] : [],
  chainId,
});

module.exports = {
  solidity: {
    version: "0.8.33",
    settings: {
      optimizer: { enabled: true, runs: 10000 },
    },
  },

  // Ignition CREATE2 configuration
  ignition: {
    strategyConfig: {
      create2: {
        salt: ETOMIC_SWAP_V1_CREATE2_SALT,
      },
    },
  },

  networks: {
    // Local development
    localhost: {
      url: "http://127.0.0.1:8545",
      accounts: DEPLOYER_PRIVATE_KEY ? [DEPLOYER_PRIVATE_KEY] : undefined,
    },

    // ============ TESTNETS ============
    sepolia: networkConfig(11155111, "SEPOLIA_RPC_URL"),
    bscTestnet: networkConfig(97, "BSC_TESTNET_RPC_URL"),
    polygonMumbai: networkConfig(80001, "POLYGON_MUMBAI_RPC_URL"),
    avalancheFuji: networkConfig(43113, "AVALANCHE_TESTNET_RPC_URL"),
    fantomTestnet: networkConfig(4002, "FANTOM_TESTNET_RPC_URL"),

    // ============ MAINNETS ============
    // Primary (Phase 1)
    mainnet: networkConfig(1, "MAINNET_RPC_URL"),

    // Future deployments (Phase 2+)
    bsc: networkConfig(56, "BSC_RPC_URL"),
    polygon: networkConfig(137, "POLYGON_RPC_URL"),
    avalanche: networkConfig(43114, "AVALANCHE_RPC_URL"),
    arbitrumOne: networkConfig(42161, "ARBITRUM_RPC_URL"),
    base: networkConfig(8453, "BASE_RPC_URL"),
    kcc: networkConfig(321, "KCC_RPC_URL"),
    moonriver: networkConfig(1285, "MOONRIVER_RPC_URL"),
    moonbeam: networkConfig(1284, "MOONBEAM_RPC_URL"),
    fantom: networkConfig(250, "FANTOM_RPC_URL"),
    harmony: networkConfig(1666600000, "HARMONY_RPC_URL"),
    etc: networkConfig(61, "ETC_RPC_URL"),
    rsk: networkConfig(30, "RSK_RPC_URL"),
    ewc: networkConfig(246, "EWC_RPC_URL"),
    smartbch: networkConfig(10000, "SMARTBCH_RPC_URL"),
    ubiq: networkConfig(8, "UBIQ_RPC_URL"),
    // Note: Qtum EVM (3888) may need special handling
  },

  etherscan: {
    apiKey: {
      // Testnets
      sepolia: process.env.ETHERSCAN_API_KEY,
      bscTestnet: process.env.BSCSCAN_API_KEY,
      polygonMumbai: process.env.POLYGONSCAN_API_KEY,
      avalancheFujiTestnet: process.env.SNOWTRACE_API_KEY,
      ftmTestnet: process.env.FTMSCAN_API_KEY,

      // Mainnets
      mainnet: process.env.ETHERSCAN_API_KEY,
      bsc: process.env.BSCSCAN_API_KEY,
      polygon: process.env.POLYGONSCAN_API_KEY,
      avalanche: process.env.SNOWTRACE_API_KEY,
      arbitrumOne: process.env.ARBISCAN_API_KEY,
      base: process.env.BASESCAN_API_KEY,
      moonriver: process.env.MOONSCAN_API_KEY,
      moonbeam: process.env.MOONSCAN_API_KEY,
      opera: process.env.FTMSCAN_API_KEY, // Fantom
    },
    customChains: [
      // Add custom chains that aren't in hardhat-verify by default
      {
        network: "base",
        chainId: 8453,
        urls: {
          apiURL: "https://api.basescan.org/api",
          browserURL: "https://basescan.org",
        },
      },
    ],
  },
};
```

### 2.4 Environment Variables

Create `.env` file (add to `.gitignore`):

```bash
# Deployer
DEPLOYER_PRIVATE_KEY=0x...

# CREATE2 Salt (32 bytes, mined for vanity address)
ETOMIC_SWAP_V1_CREATE2_SALT=0x...

# ============ RPC URLs ============
# Testnets
SEPOLIA_RPC_URL=https://sepolia.infura.io/v3/YOUR_KEY
BSC_TESTNET_RPC_URL=https://data-seed-prebsc-1-s1.binance.org:8545
POLYGON_MUMBAI_RPC_URL=https://rpc-mumbai.maticvigil.com
AVALANCHE_TESTNET_RPC_URL=https://api.avax-test.network/ext/bc/C/rpc
FANTOM_TESTNET_RPC_URL=https://rpc.testnet.fantom.network

# Mainnets - Primary (Phase 1)
MAINNET_RPC_URL=https://mainnet.infura.io/v3/YOUR_KEY

# Mainnets - Future (Phase 2+)
BSC_RPC_URL=https://bsc-dataseed.binance.org
POLYGON_RPC_URL=https://polygon-mainnet.infura.io/v3/YOUR_KEY
AVALANCHE_RPC_URL=https://api.avax.network/ext/bc/C/rpc
ARBITRUM_RPC_URL=https://arb1.arbitrum.io/rpc
BASE_RPC_URL=https://mainnet.base.org
KCC_RPC_URL=https://rpc-mainnet.kcc.network
MOONRIVER_RPC_URL=https://rpc.api.moonriver.moonbeam.network
MOONBEAM_RPC_URL=https://rpc.api.moonbeam.network
FANTOM_RPC_URL=https://rpcapi.fantom.network
HARMONY_RPC_URL=https://api.harmony.one
ETC_RPC_URL=https://etc.rivet.link
RSK_RPC_URL=https://public-node.rsk.co
EWC_RPC_URL=https://rpc.energyweb.org
SMARTBCH_RPC_URL=https://smartbch.greyh.at
UBIQ_RPC_URL=https://rpc.octano.dev

# ============ BLOCK EXPLORER API KEYS ============
ETHERSCAN_API_KEY=...
BSCSCAN_API_KEY=...
POLYGONSCAN_API_KEY=...
SNOWTRACE_API_KEY=...
ARBISCAN_API_KEY=...
BASESCAN_API_KEY=...
MOONSCAN_API_KEY=...
FTMSCAN_API_KEY=...
```

---

## 3. CREATE2 Configuration

### 3.1 How CREATE2 Works with CreateX

Hardhat Ignition uses the **CreateX factory** for CREATE2 deployments. The final address is computed as:

```
address = keccak256(0xff ++ CreateX ++ guardedSalt ++ keccak256(initCode))[12:]
```

Where `guardedSalt` depends on the salt format (see below).

### 3.2 Salt Format for Cross-Chain Deployment

To get the **same address across all chains** with **front-run protection**:

| Bytes | Content | Purpose |
|-------|---------|---------|
| 0-19 | Deployer address | Permissioned deploy (only this address can use this salt) |
| 20 | `0x00` | No cross-chain protection (same address on all chains) |
| 21-31 | Mined entropy | Found via vanity mining |

**Example salt structure**:
```
0x<deployer_address_20_bytes><00><mined_entropy_11_bytes>
```

### 3.3 Why Permissioned Salt?

- **Without permissioning**: Anyone could deploy to your target address first on a chain you haven't deployed to yet
- **With permissioning** (first 20 bytes = deployer): CreateX checks `msg.sender` matches, preventing front-running

---

## 4. Vanity Address Mining

### 4.1 Target Pattern

```
0x61eec...d3f1
    ^^^^^   ^^^^
    GLEEC   DEFI (leetspeak)
```

- Prefix `61eec` = "GLEEC" (6=G, 1=L, eec=EEC)
- Suffix `d3f1` = "DEFI" (d=D, 3=E, f=F, 1=I)
- Difficulty: 1 in 68.7 billion (16^9)

### 4.2 Tool: createXcrunch

**createXcrunch** is purpose-built for CreateX vanity mining:
- Repository: https://github.com/HrikB/createXcrunch
- Supports CreateX's `_guard()` salt transformation
- Uses OpenCL for GPU acceleration
- Only needs public inputs (no private keys)

### 4.3 Mining on Vast.ai (Recommended)

Since M1 Max doesn't have NVIDIA CUDA, use cloud GPU for faster mining.

#### Step 1: Rent GPU on Vast.ai

1. Go to https://vast.ai
2. Search for instance with RTX 3080/3090/4090
3. Select Ubuntu with Docker support
4. Rent (~$0.30-0.50/hour)

#### Step 2: Setup on Cloud Instance

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone and build createXcrunch
git clone https://github.com/HrikB/createXcrunch.git
cd createXcrunch
cargo build --release
```

#### Step 3: Get Init Code Hash

First, compile the contract locally:

```bash
# On your local machine
cd etomic-swap
npx hardhat compile

# Get init code hash
node -e "
const fs = require('fs');
const { keccak256 } = require('ethers');
const artifact = JSON.parse(fs.readFileSync('artifacts/contracts/EtomicSwap.sol/EtomicSwap.json'));
console.log('Init Code Hash:', keccak256(artifact.bytecode));
"
```

#### Step 4: Run Mining

```bash
# On vast.ai instance
./target/release/createxcrunch create2 \
  --caller 0xYOUR_DEPLOYER_ADDRESS \
  --crosschain 0 \
  --init-code-hash 0xYOUR_INIT_CODE_HASH \
  --matching 61eecXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXd3f1
```

#### Expected Time on Cloud GPU

| GPU | Speed | Expected Time |
|-----|-------|---------------|
| RTX 4090 | ~3B/sec | ~23 seconds |
| RTX 3080 | ~1.5B/sec | ~46 seconds |
| RTX 3070 | ~1.25B/sec | ~55 seconds |

#### Step 5: Record the Salt

When found, the tool outputs a 32-byte salt. Save it securely.

### 4.4 Mining Locally on M1 Max (Alternative)

If you prefer local mining (slower but no cloud needed):

```bash
# Clone and build
git clone https://github.com/HrikB/createXcrunch.git
cd createXcrunch
cargo build --release

# Run (may take 20-60 minutes)
./target/release/createxcrunch create2 \
  --caller 0xYOUR_DEPLOYER_ADDRESS \
  --crosschain 0 \
  --init-code-hash 0xYOUR_INIT_CODE_HASH \
  --matching 61eecXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXd3f1
```

---

## 5. Deployment Process

### 5.1 Pre-Deployment Checklist

- [ ] Contract compiled with correct settings (Solidity 0.8.33, optimizer runs=10000)
- [ ] Salt mined and verified (see Section 6)
- [ ] Deployer wallet funded (start with Sepolia testnet, then Ethereum mainnet)
- [ ] Environment variables set (at minimum: `DEPLOYER_PRIVATE_KEY`, `ETOMIC_SWAP_V1_CREATE2_SALT`, `SEPOLIA_RPC_URL`, `MAINNET_RPC_URL`, `ETHERSCAN_API_KEY`)
- [ ] CreateX factory verified on target network

### 5.2 Deploy to Sepolia (Test First!)

```bash
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network sepolia \
  --strategy create2 \
  --verify
```

### 5.3 Verify Vanity Address

Check that deployed address matches expected pattern:
- Starts with `0x61eec`
- Ends with `d3f1`

If it doesn't match, the salt is wrong - do NOT proceed to mainnet!

### 5.4 Deploy to Ethereum Mainnet (Phase 1)

```bash
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network mainnet \
  --strategy create2 \
  --verify
```

**After successful deployment:**
1. Record the contract address
2. Verify on Etherscan
3. Update coins repo (see Phase 6 in main plan)

### 5.5 Future Deployments (Phase 2+)

When ready to deploy to other networks, use the same command with different network:

```bash
# High-priority networks (most ERC20 tokens)
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network bsc --strategy create2 --verify
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network polygon --strategy create2 --verify
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network avalanche --strategy create2 --verify
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network arbitrumOne --strategy create2 --verify

# Other networks
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network base --strategy create2 --verify
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network kcc --strategy create2 --verify
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network moonriver --strategy create2 --verify
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network fantom --strategy create2 --verify
# ... etc
```

> **Important**: Before deploying to each network:
> 1. Verify CreateX factory exists at `0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed`
> 2. Fund deployer wallet with native token for gas
> 3. Get block explorer API key for verification

### 5.6 Package.json Scripts (Optional)

Add convenience scripts:

```json
{
  "scripts": {
    "deploy:v1:sepolia": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network sepolia --strategy create2 --verify",
    "deploy:v1:mainnet": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network mainnet --strategy create2 --verify",
    "deploy:v1:bsc": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network bsc --strategy create2 --verify",
    "deploy:v1:polygon": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network polygon --strategy create2 --verify",
    "deploy:v1:avalanche": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network avalanche --strategy create2 --verify",
    "deploy:v1:arbitrum": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network arbitrumOne --strategy create2 --verify",
    "deploy:v1:base": "hardhat ignition deploy ignition/modules/EtomicSwapV1.js --network base --strategy create2 --verify"
  }
}
```

---

## 6. Verification

### 6.1 Verify Salt Before Deployment

Create `scripts/verify-createx-create2.mjs`:

```javascript
import fs from "node:fs";
import path from "node:path";
import { ethers } from "ethers";

// CreateX factory address (same on all networks)
const DEFAULT_CREATEX = "0xba5Ed099633D3B313e4D5F7bdc1305d3c28ba5Ed";

// Parse CLI args
function getArg(name) {
  const idx = process.argv.indexOf(`--${name}`);
  return idx !== -1 ? process.argv[idx + 1] : undefined;
}

const deployer = getArg("deployer");
const saltHex = getArg("salt");
const chainIdStr = getArg("chainId");
const createX = getArg("createX") ?? DEFAULT_CREATEX;

if (!deployer || !saltHex) {
  console.error("Usage: node scripts/verify-createx-create2.mjs --deployer 0x... --salt 0x... [--chainId 1]");
  process.exit(1);
}

const chainId = chainIdStr ? BigInt(chainIdStr) : 1n;

// Load EtomicSwap bytecode
const artifactPath = path.resolve("artifacts/contracts/EtomicSwap.sol/EtomicSwap.json");
if (!fs.existsSync(artifactPath)) {
  console.error(`Artifact not found. Run: npx hardhat compile`);
  process.exit(1);
}

const artifact = JSON.parse(fs.readFileSync(artifactPath, "utf8"));
const initCode = artifact.bytecode;
const initCodeHash = ethers.keccak256(initCode);

// Replicate CreateX _guard() logic
function guardSalt_CreateX(rawSaltHex, caller, chainIdBigInt) {
  const salt = ethers.hexlify(ethers.zeroPadValue(rawSaltHex, 32));
  const saltBytes = ethers.getBytes(salt);

  const first20 = saltBytes.slice(0, 20);
  const byte21 = saltBytes[20];

  const first20Addr = ethers.getAddress(ethers.hexlify(first20));
  const callerAddr = ethers.getAddress(caller);

  const isMsgSender = first20Addr === callerAddr;
  const isZeroAddr = first20Addr === ethers.ZeroAddress;

  const flagTrue = byte21 === 0x01;
  const flagFalse = byte21 === 0x00;

  const abi = ethers.AbiCoder.defaultAbiCoder();

  // MsgSender + True => keccak256(abi.encode(msg.sender, chainid, salt))
  if (isMsgSender && flagTrue) {
    return ethers.keccak256(
      abi.encode(["address", "uint256", "bytes32"], [callerAddr, chainIdBigInt, salt])
    );
  }

  // MsgSender + False => keccak256(bytes32(uint160(msg.sender)) || salt)
  if (isMsgSender && flagFalse) {
    const a = ethers.zeroPadValue(callerAddr, 32);
    return ethers.keccak256(ethers.concat([a, salt]));
  }

  // ZeroAddress + True => keccak256(bytes32(chainid) || salt)
  if (isZeroAddr && flagTrue) {
    const a = ethers.zeroPadValue(ethers.toBeHex(chainIdBigInt), 32);
    return ethers.keccak256(ethers.concat([a, salt]));
  }

  // Default: keccak256(abi.encode(salt))
  return ethers.keccak256(abi.encode(["bytes32"], [salt]));
}

// Compute address
const guardedSalt = guardSalt_CreateX(saltHex, deployer, chainId);
const predicted = ethers.getCreate2Address(
  ethers.getAddress(createX),
  guardedSalt,
  initCodeHash
);

const targetPrefix = "61eec";
const targetSuffix = "d3f1";
const addrNo0x = predicted.slice(2).toLowerCase();

console.log("=".repeat(60));
console.log("CreateX Address Verification");
console.log("=".repeat(60));
console.log("CreateX:       ", ethers.getAddress(createX));
console.log("Deployer:      ", ethers.getAddress(deployer));
console.log("Raw Salt:      ", ethers.hexlify(ethers.zeroPadValue(saltHex, 32)));
console.log("Guarded Salt:  ", guardedSalt);
console.log("InitCodeHash:  ", initCodeHash);
console.log("Chain ID:      ", chainId.toString());
console.log("-".repeat(60));
console.log("Predicted Addr:", predicted);
console.log("-".repeat(60));
console.log(`Prefix Match (${targetPrefix}):`, addrNo0x.startsWith(targetPrefix) ? "YES" : "NO");
console.log(`Suffix Match (${targetSuffix}):`, addrNo0x.endsWith(targetSuffix) ? "YES" : "NO");
console.log("=".repeat(60));
```

Run verification:

```bash
npx hardhat compile
node scripts/verify-createx-create2.mjs \
  --deployer 0xYOUR_DEPLOYER_ADDRESS \
  --salt 0xYOUR_MINED_SALT \
  --chainId 1
```

### 6.2 Post-Deployment Verification

After deployment, verify on block explorers:

```bash
# If --verify flag didn't work, verify manually
npx hardhat verify --network mainnet <deployed_address>
```

---

## 7. Security Considerations

### 7.1 Vanity Mining Security

**This is NOT like the Profanity vulnerability** because:
- Profanity generated weak **private keys** for EOAs
- Here we're mining a **salt**, not a key
- Salt mining only needs public inputs (no private keys involved)

**Best practices**:
1. **Never provide private keys** to mining tools
2. **Build from source** - clone repo, checkout specific commit
3. **Run with networking disabled** (optional extra security)
4. **Verify independently** - use the verification script to confirm salt produces expected address

### 7.2 CreateX Considerations

From Hardhat docs:
> "Because Create2 uses an external factory (CreateX), be mindful of security considerations when using it on mainnet."

**Mitigations**:
- Deploy to Sepolia first, verify everything works
- Use permissioned salt (first 20 bytes = deployer address) to prevent front-running
- Verify CreateX is deployed on target network before deploying

### 7.3 Deployment Wallet Security

- Use a dedicated deployment wallet (not your main wallet)
- Only fund with enough ETH for deployment gas
- Consider hardware wallet for mainnet deployments
- Never commit private keys to git

---

## Appendix: DEPLOYMENTS.md Template

After deployment, create `DEPLOYMENTS.md` in etomic-swap repo:

```markdown
# EtomicSwap V1 (SafeERC20) Deployments

Contract: `0x61eec...d3f1` (GLEEC...DEFI vanity address)

## Deployment Details

- **Contract**: EtomicSwap.sol with SafeERC20
- **Compiler**: Solidity 0.8.33
- **Optimizer**: Enabled, runs=10000
- **CREATE2 Salt**: `0x...`
- **Deployer**: `0x...`
- **Git Commit**: `abc123`

## Deployed Networks

### Phase 1 (Initial)

| Network | Chain ID | Address | Tx Hash | Explorer |
|---------|----------|---------|---------|----------|
| Ethereum | 1 | `0x61eec...d3f1` | `0x...` | [Etherscan](link) |

### Phase 2+ (Future)

| Network | Chain ID | Address | Tx Hash | Explorer |
|---------|----------|---------|---------|----------|
| BSC | 56 | `0x61eec...d3f1` | - | - |
| Polygon | 137 | `0x61eec...d3f1` | - | - |
| Avalanche | 43114 | `0x61eec...d3f1` | - | - |
| Arbitrum | 42161 | `0x61eec...d3f1` | - | - |
| Base | 8453 | `0x61eec...d3f1` | - | - |
| KuCoin Chain | 321 | `0x61eec...d3f1` | - | - |
| Moonriver | 1285 | `0x61eec...d3f1` | - | - |
| Moonbeam | 1284 | `0x61eec...d3f1` | - | - |
| Fantom | 250 | `0x61eec...d3f1` | - | - |
| Harmony | 1666600000 | `0x61eec...d3f1` | - | - |
| Ethereum Classic | 61 | `0x61eec...d3f1` | - | - |
| RSK | 30 | `0x61eec...d3f1` | - | - |
| Energy Web | 246 | `0x61eec...d3f1` | - | - |
| SmartBCH | 10000 | `0x61eec...d3f1` | - | - |
| Ubiq | 8 | `0x61eec...d3f1` | - | - |

## coins repo Updates

After each deployment, update the corresponding file in `coins/ethereum/`:

| Coins Platform | File | swap_contract_address |
|----------------|------|----------------------|
| ETH | `coins/ethereum/ETH` | Update to new address |
| BNB | `coins/ethereum/BNB` | Update to new address |
| MATIC | `coins/ethereum/MATIC` | Update to new address |
| ... | ... | ... |
```

---

## References

- [Hardhat Ignition CREATE2 Guide](https://v2.hardhat.org/ignition/docs/guides/create2)
- [CreateX Repository](https://github.com/pcaversaccio/createx)
- [createXcrunch Repository](https://github.com/HrikB/createXcrunch)
- [Vast.ai GPU Rentals](https://vast.ai)
- [EIP-2470: Singleton Factory](https://eips.ethereum.org/EIPS/eip-2470)
