# SafeERC20 Implementation Plan for EtomicSwap V1

## Summary

Add SafeERC20 support to the V1 EtomicSwap contract to enable USDT and other non-standard ERC20 tokens. Use the real USDT contract from Ethereum mainnet for comprehensive docker testing.

## Problem

USDT's `transfer`/`transferFrom` don't return a boolean (ABI shows `"outputs":[]`). The V1 contract uses `require(token.transferFrom(...))` which fails because Solidity tries to decode a non-existent return value.

**Evidence from USDT ABI**:
```json
{"name":"transfer","outputs":[],...}
{"name":"transferFrom","outputs":[],...}
```

---

## Phase 0: Branch Setup

### 0.1 Create Branches

1. **etomic-swap repo**: Create branch off `dev`
   ```bash
   cd etomic-swap
   git checkout dev && git pull
   git checkout -b feat/safeerc20-v1
   ```

2. **atomicDEX-API repo**: Create branch off `dev`
   ```bash
   cd atomicDEX-API
   git checkout dev && git pull
   git checkout -b feat/safeerc20-v1-tests
   ```

---

## Phase 1: Contract Changes (etomic-swap repo)

### 1.1 Update Solidity Version

**File**: `contracts/EtomicSwap.sol`

Update pragma to latest stable:
```solidity
pragma solidity ^0.8.33;  // was 0.8.30
```

**Why**: 0.8.33 includes bugfixes (0.8.32 fixed array storage bug, 0.8.33 is hotfix for compiler error).

### 1.2 Update EtomicSwap.sol with SafeERC20 and Bug Fixes

**File**: `contracts/EtomicSwap.sol`

**Changes**:

1. Add SafeERC20 import (near other ERC20 imports):
```solidity
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
```

2. Add using directive inside contract body:
```solidity
using SafeERC20 for IERC20;
```

3. **Fix `erc20Payment()` signature** - remove `payable` modifier:
```solidity
// Before:
function erc20Payment(...) external payable {
// After:
function erc20Payment(...) external {
```
**Why**: Function doesn't use `msg.value`, so `payable` allows ETH to be stuck forever. ABI already declares it as `nonpayable`.

4. **Add zero-address check** in `erc20Payment()`:
```solidity
require(tokenAddress != address(0), "Token address cannot be zero");
```
**Why**: Prevents misuse (calling erc20Payment when ethPayment was intended). SafeERC20 would revert anyway, but this gives a clearer error.

5. Update `erc20Payment()` - replace `require(token.transferFrom(...))` with:
```solidity
IERC20(tokenAddress).safeTransferFrom(msg.sender, address(this), amount);
```

6. Update `receiverSpend()` - replace `require(token.transfer(...))` with:
```solidity
IERC20(tokenAddress).safeTransfer(msg.sender, amount);
```

7. Update `senderRefund()` - replace `require(token.transfer(...))` with:
```solidity
IERC20(tokenAddress).safeTransfer(msg.sender, amount);
```

### 1.3 Update Hardhat Optimizer Settings

**File**: `hardhat.config.js`

Update to optimize for runtime gas (cheaper swaps for users):

```javascript
module.exports = {
  solidity: {
    version: "0.8.33",
    settings: {
      optimizer: {
        enabled: true,
        runs: 10000  // Optimize for runtime, not deployment
      }
    }
  },
  networks: {
    hardhat: {
      chainId: 1337,
      host: "0.0.0.0",
      port: 8545
    }
  }
};
```

**Why runs=10000**:
- Contract deployed **once**, functions called **thousands of times**
- Higher runs = optimizer inlines code for frequently-called functions
- Costs more to deploy but saves gas on every swap
- Trade-off: ~20% higher deploy cost, ~5-10% lower per-swap cost

### 1.4 Compile Contract

```bash
cd etomic-swap
npx hardhat compile
```

Extract **creation bytecode** (not runtime bytecode) from:
- `artifacts/contracts/EtomicSwap.sol/EtomicSwap.json`

**Important**: Remove `0x` prefix when storing bytecode (Rust uses `hex::decode`).

---

## Phase 2: Verify V2 Contracts (Audit)

Verify these already use SafeERC20 correctly:

- `EtomicSwapMakerV2.sol` - Uses `safeTransferFrom`/`safeTransfer`
- `EtomicSwapTakerV2.sol` - Uses `safeTransferFrom`/`safeTransfer`

**Expected**: No changes needed.

---

## Phase 3: Update atomicDEX-API

### 3.1 Update Contract Bytecode

**File**: `mm2src/mm2_test_helpers/contract_bytes/swap_contract_bytes`

Replace with new compiled **creation bytecode** from EtomicSwap.json (no `0x` prefix).

### 3.2 Add Real USDT Contract Bytecode

**File**: `mm2src/mm2_test_helpers/contract_bytes/usdt_contract_bytes` (new)

Use the **real USDT contract** from Ethereum mainnet for testing. Get from Etherscan:
- URL: https://etherscan.io/address/0xdac17f958d2ee523a2206206994597c13d831ec7#code
- Copy "Contract Creation Code" section
- Remove `0x` prefix
- Save to file

**Why real USDT**: Tests against actual production contract behavior, not a mock approximation.

**Constructor parameters already encoded**:
- Initial supply: 100 billion units
- Name: "Tether USD"
- Symbol: "USDT"
- Decimals: 6

### 3.3 Add USDT ABI

**File**: `mm2src/mm2_test_helpers/contract_bytes/usdt_abi.json` (new)

Copy from Etherscan "Contract ABI" section. Key functions:
- `transfer(address,uint256)` - no return value
- `transferFrom(address,address,uint256)` - no return value
- `approve(address,uint256)` - returns nothing (but standard expects bool)
- `balanceOf(address)` - returns uint256
- `issue(uint256)` - owner-only mint function (useful for tests)

### 3.4 Update Docker Test Helpers

**File**: `mm2src/mm2_main/tests/docker_tests/helpers/eth.rs`

Add:
1. `USDT_BYTES` constant (from `usdt_contract_bytes` file)
2. `GETH_USDT_CONTRACT` OnceLock static for deployed address
3. `geth_usdt_contract()` getter function
4. `usdt_checksum()` helper for checksummed address
5. `fill_usdt()` funding function (transfer from deployer)
6. Deploy USDT contract in `init_geth_node()`
7. `usdt_coin_with_random_privkey()` helper

**Critical**: Add dedicated config helper with correct decimals:
```rust
fn usdt_dev_conf(contract_address: &str) -> Json {
    json!({
        "coin": "USDT",
        "name": "usdt",
        "decimals": 6,  // USDT uses 6 decimals, not 8 like other test tokens
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": contract_address
            }
        },
        // ... other fields
    })
}
```

### 3.5 Add Docker Tests

**File**: `mm2src/mm2_main/tests/docker_tests/eth_docker_tests.rs`

Add tests:
1. `test_usdt_maker_payment_send_and_spend()` - Payment + spend flow with USDT
2. `test_usdt_maker_payment_refund()` - Payment + refund flow with USDT
3. `test_usdt_get_token_decimals()` - **Verify `get_token_decimals()` works with USDT**

These mirror existing `erc20_maker_payment` tests but use USDT contract.

**Important**: Test #3 verifies that mm2's `get_token_decimals()` function correctly handles USDT's non-standard `decimals()` return type (uint256 instead of uint8). This ensures on-chain decimals detection works even though config provides the value.

---

## Phase 4: Verification

### 4.1 Run Hardhat Tests (etomic-swap)
```bash
cd etomic-swap
npm test
```

### 4.2 Run Clippy (atomicDEX-API)
```bash
cargo clippy -p mm2_main --features docker-tests-eth -- -D warnings
cargo clippy -p mm2_test_helpers -- -D warnings
```

### 4.3 Run Docker Tests
```bash
export BOB_PASSPHRASE="also shoot benefit prefer juice shell elder veteran woman mimic image kidney"
export ALICE_PASSPHRASE="spice describe gravity federal blast come thank unfair canal monkey style afraid"

# New USDT tests
cargo test --test docker_tests_main --features docker-tests-eth -- usdt_maker_payment --nocapture

# Verify no regressions on standard ERC20
cargo test --test docker_tests_main --features docker-tests-eth -- erc20_maker_payment --nocapture
```

---

## Phase 4.5: Security Review

**Mandatory review before any deployment (testnet or mainnet).**

This phase is performed manually before proceeding to deployment. The checklist is intentionally open-ended to allow for more extensive review if needed.

### Security Checklist

#### Contract Changes
- [ ] SafeERC20 usage is correct (no double-wrapping, proper imports)
- [ ] No new reentrancy vectors introduced
- [ ] State changes occur before external calls (CEI pattern maintained)
- [ ] All existing require() checks still in place
- [ ] No accidental removal of access controls

#### Optimizer & Compiler
- [ ] Optimizer settings don't change contract behavior
- [ ] Solidity version 0.8.33 compatible with OpenZeppelin 5.x
- [ ] No compiler warnings in build output

#### Deployment Security
- [ ] Verify bytecode matches source (compare compiled output)
- [ ] Constructor parameters correct (none for EtomicSwap)
- [ ] Deployment wallet security verified
- [ ] Sufficient gas for deployment
- [ ] Etherscan verification planned

#### Testing Verification
- [ ] All hardhat tests pass
- [ ] All docker tests pass (especially new USDT tests)
- [ ] Gas comparison report reviewed (before vs after)
- [ ] No regressions in existing ERC20 swap tests

#### Additional Review (Optional)
- [ ] Third-party code review
- [ ] Formal security audit (if scope warrants)
- [ ] Testnet validation period completed

**Note**: This checklist may be extended during the review process.

---

## Phase 5: Contract Deployment (Multi-Chain)

> **Detailed deployment guide**: See [etomic-swap-v1-deployment.md](./etomic-swap-v1-deployment.md) for complete instructions including:
> - Hardhat Ignition setup
> - CREATE2 deterministic deployment
> - Vanity address mining (`0x61eec...d3f1` = GLEEC...DEFI)
> - Multi-network deployment (Ethereum, Polygon, BSC, Arbitrum)

### Summary

Deployment uses **Hardhat Ignition** with **CREATE2** strategy for:
- Same contract address across all EVM chains
- Vanity address: `0x61eec...d3f1` (GLEEC...DEFI)
- Front-run protection via permissioned salt

### 5.1 Deploy to Sepolia (Test)

```bash
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network sepolia \
  --strategy create2 \
  --verify
```

### 5.2 Verify Vanity Address

Confirm deployed address:
- Starts with `0x61eec`
- Ends with `d3f1`

### 5.3 Deploy to Production Networks

```bash
# Ethereum Mainnet
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network mainnet --strategy create2 --verify

# Polygon
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network polygon --strategy create2 --verify

# BSC
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network bsc --strategy create2 --verify

# Arbitrum
npx hardhat ignition deploy ignition/modules/EtomicSwapV1.js \
  --network arbitrumOne --strategy create2 --verify
```

### 5.4 Record Deployments

Update `DEPLOYMENTS.md` with addresses and verification links.

---

## Phase 6: Post-Deployment (coins repo)

**After contract is deployed to Ethereum mainnet:**

1. **coins repo**: Create branch off `master`
   ```bash
   cd coins
   git checkout master && git pull
   git checkout -b feat/usdt-enable-trading
   ```

2. Update **Ethereum platform** swap contract:
   - File: `coins/ethereum/ETH` (JSON file - **NOT** the main `coins` file)
   - This file contains `swap_contract_address` and `fallback_swap_contract`
   - Update `swap_contract_address` with new Ethereum mainnet address
   - Move old address to `fallback_swap_contract` (for in-progress swaps on other tokens)

   Example change:
   ```json
   {
     "swap_contract_address": "0xNEW_SAFEERC20_CONTRACT",
     "fallback_swap_contract": "0xOLD_CONTRACT_ADDRESS"
   }
   ```

3. Enable USDT trading:
   - File: `coins` (main coins JSON file)
   - Find `USDT-ERC20` entry
   - Remove `"wallet_only": true` field
   - Note: USDT has always been `wallet_only: true`, so no existing USDT swaps exist

4. Add TODO in PR description for other EVM chains:
   ```
   TODO: Deploy SafeERC20-compatible swap contract to:
   - Polygon
   - BSC
   - Arbitrum
   - Other EVM chains
   ```

5. Create PR to enable USDT trading on Ethereum

---

## Files to Modify

### etomic-swap repo:
| File | Action |
|------|--------|
| `contracts/EtomicSwap.sol` | Upgrade to 0.8.33, add SafeERC20, remove payable from erc20Payment, add tokenAddress check |
| `hardhat.config.js` | Add optimizer runs=10000, Ignition CREATE2 config, multi-network configs |
| `ignition/modules/EtomicSwapV1.js` | Create (new) - Hardhat Ignition deployment module |
| `scripts/verify-createx-create2.mjs` | Create (new) - Verify vanity address before deployment |
| `.env.example` | Create (new) - Environment variables template |

### atomicDEX-API repo:
| File | Action |
|------|--------|
| `mm2src/mm2_test_helpers/contract_bytes/swap_contract_bytes` | Update bytecode |
| `mm2src/mm2_test_helpers/contract_bytes/usdt_contract_bytes` | Create (new) - real USDT bytecode from Etherscan |
| `mm2src/mm2_test_helpers/contract_bytes/usdt_abi.json` | Create (new) - USDT ABI from Etherscan |
| `mm2src/mm2_main/tests/docker_tests/helpers/eth.rs` | Add USDT deployment + helpers + `usdt_dev_conf()` |
| `mm2src/mm2_main/tests/docker_tests/eth_docker_tests.rs` | Add USDT tests |

### coins repo (post-deployment):
| File | Action |
|------|--------|
| `coins/ethereum/ETH` | Update `swap_contract_address`, move old to `fallback_swap_contract` |
| `coins` | Remove `"wallet_only": true` from USDT-ERC20 entry |

---

## Notes

- **ABI unchanged**: EtomicSwap function signatures don't change, so `swap_contract_abi.json` stays the same
- **V2 already compatible**: V2 contracts use SafeERC20 - no changes needed
- **coins repo PR**: Only after Ethereum mainnet deployment is confirmed
- **Other EVM chains**: Left as TODO in coins PR for future deployment
- **Real USDT for tests**: Using actual mainnet USDT contract bytecode ensures tests match production behavior exactly
- **Solidity 0.8.33**: Latest stable version with bugfixes (array storage bug fix in 0.8.32)
- **Optimizer runs=10000**: Optimizes for runtime gas (cheaper per-swap cost for users, higher one-time deployment cost)

---

## Follow-up Work (TODO)

### Watchers Swap Contract

The watchers swap contract (`WATCHERS_SWAP_CONTRACT_BYTES`) has the same non-SafeERC20 issue. The contract source is on the `ethereum-swap-watchers` branch in the etomic-swap repo.

**Confirmed issue**: The watchers contract uses `require(token.transferFrom(...))` in both:
- `erc20Payment()`
- `erc20PaymentReward()`

This should be fixed in a follow-up PR:

1. Checkout `ethereum-swap-watchers` branch in etomic-swap repo
2. **Apply same changes as this plan**:
   - Upgrade Solidity to 0.8.33
   - Add SafeERC20 to all ERC20 transfer points (including `*Reward` functions)
   - Update hardhat.config.js with optimizer runs=10000
3. Compile and update `watchers_swap_contract_bytes` in atomicDEX-API
4. Update `swap_contract_abi.json` if function signatures change (unlikely with SafeERC20)
5. **Run same Security Review checklist** (Phase 4.5)
6. Create separate PR for watcher contract changes

**Tracking**: Add TODO comment in the main PR for visibility.

### USDT Approval Behavior (Known Limitation)

USDT has a non-standard `approve()` function that requires allowance to be 0 before setting a new non-zero value:
```solidity
// USDT's approve check:
if ((_value != 0) && (allowed[msg.sender][_spender] != 0)) throw;
```

**mm2's behavior**: Uses `U256::MAX` (infinite) approval on first swap. Since this never decreases (unless externally modified), subsequent swaps skip the approve call entirely. This pattern **works correctly** for sequential swaps.

**Edge case**: If allowance is externally reduced to a non-zero value (e.g., by another dApp), mm2's next approve call would revert. This is an acceptable risk given:
- mm2 controls its own approvals
- External modification is rare
- First-time approval (0 → MAX) always works

**Reference**: [USDT approval requirement](https://github.com/UniverseXYZ/Universe-Marketplace/issues/29)

---

## Appendix: USDT Contract Data from Etherscan

Source: https://etherscan.io/address/0xdac17f958d2ee523a2206206994597c13d831ec7#code

### USDT ABI

```json
[{"constant":true,"inputs":[],"name":"name","outputs":[{"name":"","type":"string"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[{"name":"_upgradedAddress","type":"address"}],"name":"deprecate","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":false,"inputs":[{"name":"_spender","type":"address"},{"name":"_value","type":"uint256"}],"name":"approve","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[],"name":"deprecated","outputs":[{"name":"","type":"bool"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[{"name":"_evilUser","type":"address"}],"name":"addBlackList","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[],"name":"totalSupply","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[{"name":"_from","type":"address"},{"name":"_to","type":"address"},{"name":"_value","type":"uint256"}],"name":"transferFrom","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[],"name":"upgradedAddress","outputs":[{"name":"","type":"address"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[{"name":"","type":"address"}],"name":"balances","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"decimals","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"maximumFee","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"_totalSupply","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[],"name":"unpause","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[{"name":"_maker","type":"address"}],"name":"getBlackListStatus","outputs":[{"name":"","type":"bool"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[{"name":"","type":"address"},{"name":"","type":"address"}],"name":"allowed","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"paused","outputs":[{"name":"","type":"bool"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[{"name":"who","type":"address"}],"name":"balanceOf","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[],"name":"pause","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[],"name":"getOwner","outputs":[{"name":"","type":"address"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"owner","outputs":[{"name":"","type":"address"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"symbol","outputs":[{"name":"","type":"string"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[{"name":"_to","type":"address"},{"name":"_value","type":"uint256"}],"name":"transfer","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":false,"inputs":[{"name":"newBasisPoints","type":"uint256"},{"name":"newMaxFee","type":"uint256"}],"name":"setParams","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":false,"inputs":[{"name":"amount","type":"uint256"}],"name":"issue","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":false,"inputs":[{"name":"amount","type":"uint256"}],"name":"redeem","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[{"name":"_owner","type":"address"},{"name":"_spender","type":"address"}],"name":"allowance","outputs":[{"name":"remaining","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[],"name":"basisPointsRate","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":true,"inputs":[{"name":"","type":"address"}],"name":"isBlackListed","outputs":[{"name":"","type":"bool"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[{"name":"_clearedUser","type":"address"}],"name":"removeBlackList","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[],"name":"MAX_UINT","outputs":[{"name":"","type":"uint256"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[{"name":"newOwner","type":"address"}],"name":"transferOwnership","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":false,"inputs":[{"name":"_blackListedUser","type":"address"}],"name":"destroyBlackFunds","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"inputs":[{"name":"_initialSupply","type":"uint256"},{"name":"_name","type":"string"},{"name":"_symbol","type":"string"},{"name":"_decimals","type":"uint256"}],"payable":false,"stateMutability":"nonpayable","type":"constructor"},{"anonymous":false,"inputs":[{"indexed":false,"name":"amount","type":"uint256"}],"name":"Issue","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"name":"amount","type":"uint256"}],"name":"Redeem","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"name":"newAddress","type":"address"}],"name":"Deprecate","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"name":"feeBasisPoints","type":"uint256"},{"indexed":false,"name":"maxFee","type":"uint256"}],"name":"Params","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"name":"_blackListedUser","type":"address"},{"indexed":false,"name":"_balance","type":"uint256"}],"name":"DestroyedBlackFunds","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"name":"_user","type":"address"}],"name":"AddedBlackList","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"name":"_user","type":"address"}],"name":"RemovedBlackList","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"name":"owner","type":"address"},{"indexed":true,"name":"spender","type":"address"},{"indexed":false,"name":"value","type":"uint256"}],"name":"Approval","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"name":"from","type":"address"},{"indexed":true,"name":"to","type":"address"},{"indexed":false,"name":"value","type":"uint256"}],"name":"Transfer","type":"event"},{"anonymous":false,"inputs":[],"name":"Pause","type":"event"},{"anonymous":false,"inputs":[],"name":"Unpause","type":"event"}]
```

**Key observations**:
- `transfer` has `"outputs":[]` - no return value
- `transferFrom` has `"outputs":[]` - no return value
- `approve` has `"outputs":[]` - also no return (non-standard)
- `issue(uint256)` - owner-only mint function for testing

### USDT Contract Creation Bytecode

**Note**: Remove `0x` prefix when saving to `usdt_contract_bytes` file.

```
606060405260008060146101000a81548160ff0219169083151502179055506000600355600060045534156200003457600080fd5b60405162002d7c38038062002d7c83398101604052808051906020019091908051820191906020018051820191906020018051906020019091905050336000806101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff160217905550836001819055508260079080519060200190620000cf9291906200017a565b508160089080519060200190620000e89291906200017a565b508060098190555083600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506000600a60146101000a81548160ff0219169083151502179055505050505062000229565b828054600181600116156101000203166002900490600052602060002090601f016020900481019282601f10620001bd57805160ff1916838001178555620001ee565b82800160010185558215620001ee579182015b82811115620001ed578251825591602001919060010190620001d0565b5b509050620001fd919062000201565b5090565b6200022691905b808211156200022257600081600090555060010162000208565b5090565b90565b612b4380620002396000396000f300606060405260043610610196576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806306fdde031461019b5780630753c30c14610229578063095ea7b3146102625780630e136b19146102a45780630ecb93c0146102d157806318160ddd1461030a57806323b872dd1461033357806326976e3f1461039457806327e235e3146103e9578063313ce56714610436578063353907141461045f5780633eaaf86b146104885780633f4ba83a146104b157806359bf1abe146104c65780635c658165146105175780635c975abb1461058357806370a08231146105b05780638456cb59146105fd578063893d20e8146106125780638da5cb5b1461066757806395d89b41146106bc578063a9059cbb1461074a578063c0324c771461078c578063cc872b66146107b8578063db006a75146107db578063dd62ed3e146107fe578063dd644f721461086a578063e47d606014610893578063e4997dc5146108e4578063e5b5019a1461091d578063f2fde38b14610946578063f3bdc2281461097f575b600080fd5b34156101a657600080fd5b6101ae6109b8565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156101ee5780820151818401526020810190506101d3565b50505050905090810190601f16801561021b5780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b341561023457600080fd5b610260600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610a56565b005b341561026d57600080fd5b6102a2600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091908035906020019091905050610b73565b005b34156102af57600080fd5b6102b7610cc1565b604051808215151515815260200191505060405180910390f35b34156102dc57600080fd5b610308600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050610cd4565b005b341561031557600080fd5b61031d610ded565b6040518082815260200191505060405180910390f35b341561033e57600080fd5b610392600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091908035906020019091905050610ebd565b005b341561039f57600080fd5b6103a761109d565b604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390f35b34156103f457600080fd5b610420600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919050506110c3565b6040518082815260200191505060405180910390f35b341561044157600080fd5b6104496110db565b6040518082815260200191505060405180910390f35b341561046a57600080fd5b6104726110e1565b6040518082815260200191505060405180910390f35b341561049357600080fd5b61049b6110e7565b6040518082815260200191505060405180910390f35b34156104bc57600080fd5b6104c46110ed565b005b34156104d157600080fd5b6104fd600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919050506111ab565b604051808215151515815260200191505060405180910390f35b341561052257600080fd5b61056d600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611201565b6040518082815260200191505060405180910390f35b341561058e57600080fd5b610596611226565b604051808215151515815260200191505060405180910390f35b34156105bb57600080fd5b6105e7600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611239565b6040518082815260200191505060405180910390f35b341561060857600080fd5b610610611348565b005b341561061d57600080fd5b610625611408565b604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390f35b341561067257600080fd5b61067a611431565b604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390f35b34156106c757600080fd5b6106cf611456565b6040518080602001828103825283818151815260200191508051906020019080838360005b8381101561070f5780820151818401526020810190506106f4565b50505050905090810190601f16801561073c5780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b341561075557600080fd5b61078a600480803573ffffffffffffffffffffffffffffffffffffffff169060200190919080359060200190919050506114f4565b005b341561079757600080fd5b6107b6600480803590602001909190803590602001909190505061169e565b005b34156107c357600080fd5b6107d96004808035906020019091905050611783565b005b34156107e657600080fd5b6107fc600480803590602001909190505061197a565b005b341561080957600080fd5b610854600480803573ffffffffffffffffffffffffffffffffffffffff1690602001909190803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611b0d565b6040518082815260200191505060405180910390f35b341561087557600080fd5b61087d611c52565b6040518082815260200191505060405180910390f35b341561089e57600080fd5b6108ca600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611c58565b604051808215151515815260200191505060405180910390f35b34156108ef57600080fd5b61091b600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611c78565b005b341561092857600080fd5b610930611d91565b6040518082815260200191505060405180910390f35b341561095157600080fd5b61097d600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611db5565b005b341561098a57600080fd5b6109b6600480803573ffffffffffffffffffffffffffffffffffffffff16906020019091905050611e8a565b005b60078054600181600116156101000203166002900480601f016020809104026020016040519081016040528092919081815260200182805460018160011615610100020316600290048015610a4e5780601f10610a2357610100808354040283529160200191610a4e565b820191906000526020600020905b815481529060010190602001808311610a3157829003601f168201915b505050505081565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff16141515610ab157600080fd5b6001600a60146101000a81548160ff02191690831515021790555080600a60006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff1602179055507fcc358699805e9a8b7f77b522628c7cb9abd07d9efb86b6fb616af1609036a99e81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a150565b604060048101600036905010151515610b8b57600080fd5b600a60149054906101000a900460ff1615610cb157600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1663aee92d333385856040518463ffffffff167c0100000000000000000000000000000000000000000000000000000000028152600401808473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019350505050600060405180830381600087803b1515610c9857600080fd5b6102c65a03f11515610ca957600080fd5b505050610cbc565b610cbb838361200e565b5b505050565b600a60149054906101000a900460ff1681565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff16141515610d2f57600080fd5b6001600660008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060006101000a81548160ff0219169083151502179055507f42e160154868087d6bfdc0ca23d96a1c1cfa32f1b72ba9ba27b69b98a0d819dc81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a150565b6000600a60149054906101000a900460ff1615610eb457600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff166318160ddd6000604051602001526040518163ffffffff167c0100000000000000000000000000000000000000000000000000000000028152600401602060405180830381600087803b1515610e9257600080fd5b6102c65a03f11515610ea357600080fd5b505050604051805190509050610eba565b60015490505b90565b600060149054906101000a900460ff16151515610ed957600080fd5b600660008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900460ff16151515610f3257600080fd5b600a60149054906101000a900460ff161561108c57600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16638b477adb338585856040518563ffffffff167c0100000000000000000000000000000000000000000000000000000000028152600401808573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001828152602001945050505050600060405180830381600087803b151561107357600080fd5b6102c65a03f1151561108457600080fd5b505050611098565b6110978383836121ab565b5b505050565b600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1681565b60026020528060005260406000206000915090505481565b60095481565b60045481565b60015481565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff1614151561114857600080fd5b600060149054906101000a900460ff16151561116357600080fd5b60008060146101000a81548160ff0219169083151502179055507f7805862f689e2f13df9f062ff482ad3ad112aca9e0847911ed832e158c525b3360405160405180910390a1565b6000600660008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900460ff169050919050565b6005602052816000526040600020602052806000526040600020600091509150505481565b600060149054906101000a900460ff1681565b6000600a60149054906101000a900460ff161561133757600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff166370a08231836000604051602001526040518263ffffffff167c0100000000000000000000000000000000000000000000000000000000028152600401808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001915050602060405180830381600087803b151561131557600080fd5b6102c65a03f1151561132657600080fd5b505050604051805190509050611343565b61134082612652565b90505b919050565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff161415156113a357600080fd5b600060149054906101000a900460ff161515156113bf57600080fd5b6001600060146101000a81548160ff0219169083151502179055507f6985a02210a168e66602d3235cb6db0e70f92b3ba4d376a33c0f3d9434bff62560405160405180910390a1565b60008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff16905090565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1681565b60088054600181600116156101000203166002900480601f0160208091040260200160405190810160405280929190818152602001828054600181600116156101000203166002900480156114ec5780601f106114c1576101008083540402835291602001916114ec565b820191906000526020600020905b8154815290600101906020018083116114cf57829003601f168201915b505050505081565b600060149054906101000a900460ff1615151561151057600080fd5b600660003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900460ff1615151561156957600080fd5b600a60149054906101000a900460ff161561168f57600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16636e18980a3384846040518463ffffffff167c0100000000000000000000000000000000000000000000000000000000028152600401808473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019350505050600060405180830381600087803b151561167657600080fd5b6102c65a03f1151561168757600080fd5b50505061169a565b611699828261269b565b5b5050565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff161415156116f957600080fd5b60148210151561170857600080fd5b60328110151561171757600080fd5b81600381905550611736600954600a0a82612a0390919063ffffffff16565b6004819055507fb044a1e409eac5c48e5af22d4af52670dd1a99059537a78b31b48c6500a6354e600354600454604051808381526020018281526020019250505060405180910390a15050565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff161415156117de57600080fd5b60015481600154011115156117f257600080fd5b600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205481600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054011115156118c257600080fd5b80600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008282540192505081905550806001600082825401925050819055507fcb8241adb0c3fdb35b70c24ce35c5eb0c17af7431c99f827d44a445ca624176a816040518082815260200191505060405180910390a150565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff161415156119d557600080fd5b80600154101515156119e657600080fd5b80600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205410151515611a5557600080fd5b8060016000828254039250508190555080600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020600082825403925050819055507f702d5967f45f6513a38ffc42d6ba9bf230bd40e8f53b16363c7eb4fd2deb9a44816040518082815260200191505060405180910390a150565b6000600a60149054906101000a900460ff1615611c3f57600a60009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1663dd62ed3e84846000604051602001526040518363ffffffff167c0100000000000000000000000000000000000000000000000000000000028152600401808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200192505050602060405180830381600087803b1515611c1d57600080fd5b6102c65a03f11515611c2e57600080fd5b505050604051805190509050611c4c565b611c498383612a3e565b90505b92915050565b60035481565b60066020528060005260406000206000915054906101000a900460ff1681565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff16141515611cd357600080fd5b6000600660008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060006101000a81548160ff0219169083151502179055507fd7e9ec6e6ecd65492dce6bf513cd6867560d49544421d0783ddf06e76c24470c81604051808273ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200191505060405180910390a150565b7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff81565b6000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff16141515611e1057600080fd5b600073ffffffffffffffffffffffffffffffffffffffff168173ffffffffffffffffffffffffffffffffffffffff16141515611e8757806000806101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908373ffffffffffffffffffffffffffffffffffffffff1602179055505b50565b60008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff16141515611ee757600080fd5b600660008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060009054906101000a900460ff161515611f3f57600080fd5b611f4882611239565b90506000600260008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550806001600082825403925050819055507f61e6e66b0d6339b2980aecc6ccc0039736791f0ccde9ed512e789a7fbdd698c68282604051808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019250505060405180910390a15050565b60406004810160003690501015151561202657600080fd5b600082141580156120b457506000600560003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205414155b1515156120c057600080fd5b81600560003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508273ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925846040518082815260200191505060405180910390a3505050565b60008060006060600481016000369050101515156121c857600080fd5b600560008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054935061227061271061226260035488612a0390919063ffffffff16565b612ac590919063ffffffff16565b92506004548311156122825760045492505b7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff84101561233e576122bd8585612ae090919063ffffffff16565b600560008973ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055505b6123518386612ae090919063ffffffff16565b91506123a585600260008a73ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054612ae090919063ffffffff16565b600260008973ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000208190555061243a82600260008973ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054612af990919063ffffffff16565b600260008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000208190555060008311156125e4576124f983600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054612af990919063ffffffff16565b600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168773ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef856040518082815260200191505060405180910390a35b8573ffffffffffffffffffffffffffffffffffffffff168773ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef846040518082815260200191505060405180910390a350505050505050565b6000600260008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020549050919050565b6000806040600481016000369050101515156126b657600080fd5b6126df6127106126d160035487612a0390919063ffffffff16565b612ac590919063ffffffff16565b92506004548311156126f15760045492505b6127048385612ae090919063ffffffff16565b915061275884600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054612ae090919063ffffffff16565b600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506127ed82600260008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054612af990919063ffffffff16565b600260008773ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506000831115612997576128ac83600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054612af990919063ffffffff16565b600260008060009054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506000809054906101000a900473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef856040518082815260200191505060405180910390a35b8473ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef846040518082815260200191505060405180910390a35050505050565b6000806000841415612a185760009150612a37565b8284029050828482811515612a2957fe5b04141515612a3357fe5b8091505b5092915050565b6000600560008473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054905092915050565b6000808284811515612ad357fe5b0490508091505092915050565b6000828211151515612aee57fe5b818303905092915050565b6000808284019050838110151515612b0d57fe5b80915050929150505600a165627a7a72305820645ee12d73db47fd78ba77fa1f824c3c8f9184061b3b10386beb4dc9236abb280029000000000000000000000000000000000000000000000000000000174876e800000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000c00000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a546574686572205553440000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000045553445400000000000000000000000000000000000000000000000000000000
```

**Encoded constructor parameters** (last ~320 bytes):
- `_initialSupply`: `174876e800` hex = 100,000,000,000 decimal
  - This is in **smallest units** (like wei for ETH)
  - With decimals=6: `100,000,000,000 / 10^6 = 100,000 USDT`
- `_name`: "Tether USD" (hex: `546574686572205553440000...`)
- `_symbol`: "USDT" (hex: `5553445400000000...`)
- `_decimals`: 6

**Unit conversion reference**:
- 1 USDT = 1,000,000 smallest units (10^6)
- 100,000 USDT = 100,000,000,000 smallest units (the initial supply)

---

## Appendix: Gas Optimization References

These decisions are based on discussions from etomic-swap PRs.

### SafeERC20 Gas Cost (PR #6)
> "SafeERC20 usage increases gas consumption:
> 1. `erc20MakerPayment` uses 92,233 gas (~1,200 increase)
> 2. `spendMakerPayment` uses 50,418 gas (~1,200 increase)"

**Decision**: Worth the cost for USDT compatibility.

### Optimizer runs Parameter
- `runs=1`: Optimize for cheap deployment (inline nothing)
- `runs=200`: Balanced (Hardhat default when enabled)
- `runs=10000`: Optimize for cheap runtime (inline frequently-used code)

**Decision**: Use runs=10000 since contract is deployed once but functions called thousands of times.

### Storage Layout (PR #6)
V2 structs are carefully designed to fit in 32-byte EVM slots:
```solidity
struct MakerPayment {
    bytes20 paymentHash;  // 20 bytes
    uint32 paymentLockTime;  // 4 bytes
    MakerPaymentState state;  // 1 byte
}  // Total: 25 bytes → fits in one 32-byte slot
```

**Note**: V1 struct uses `uint64 lockTime` (8 bytes) - cannot change without breaking ABI.

### ReentrancyGuard (PR #6)
> "Making function `nonReentrant` increases gas used by ~2,500"

**Decision**: Not added - contract follows CEI (Checks-Effects-Interactions) pattern which provides reentrancy protection.

### References
- [PR #6: Trading Protocol Upgrade V2](https://github.com/GLEECBTC/etomic-swap/pull/6)
- [PR #7: NFT Swap V2](https://github.com/GLEECBTC/etomic-swap/pull/7)
- [Solidity Releases](https://github.com/ethereum/solidity/releases)
