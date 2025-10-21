use mm2_err_handle::prelude::{MmError, MmResult};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::error::WalletConnectError;

pub(crate) const SUPPORTED_PROTOCOL: &str = "irn";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WcChain {
    Eip155,
    Cosmos,
    Bip122,
}

impl FromStr for WcChain {
    type Err = MmError<WalletConnectError>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "eip155" => Ok(WcChain::Eip155),
            "cosmos" => Ok(WcChain::Cosmos),
            "bip122" => Ok(WcChain::Bip122),
            _ => MmError::err(WalletConnectError::InvalidChainId(format!(
                "chain_id not supported: {s}"
            ))),
        }
    }
}

impl AsRef<str> for WcChain {
    fn as_ref(&self) -> &str {
        match self {
            Self::Eip155 => "eip155",
            Self::Cosmos => "cosmos",
            Self::Bip122 => "bip122",
        }
    }
}

impl WcChain {
    pub(crate) fn derive_chain_id(&self, id: String) -> WcChainId {
        WcChainId {
            chain: self.clone(),
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WcChainId {
    pub chain: WcChain,
    pub id: String,
}

impl std::fmt::Display for WcChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.chain.as_ref(), self.id)
    }
}

impl WcChainId {
    pub fn new_eip155(id: String) -> Self {
        Self {
            chain: WcChain::Eip155,
            id,
        }
    }

    pub fn new_cosmos(id: String) -> Self {
        Self {
            chain: WcChain::Cosmos,
            id,
        }
    }

    pub fn try_from_str(chain_id: &str) -> MmResult<Self, WalletConnectError> {
        let sp = chain_id.split(':').collect::<Vec<_>>();
        if sp.len() != 2 {
            return MmError::err(WalletConnectError::InvalidChainId(chain_id.to_string()));
        };

        Ok(Self {
            chain: WcChain::from_str(sp[0])?,
            id: sp[1].to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum WcRequestMethods {
    CosmosSignDirect,
    CosmosSignAmino,
    CosmosGetAccounts,
    EthSignTransaction,
    EthSendTransaction,
    EthPersonalSign,
    // TODO: (remove these notes later)
    // - This method will return the pubkey of each address :D
    // - Wallets will return ALL addresses found in every purpose' derivation (44, 49, 84, 86), you need to filter for the ones the coin enabled with (or enable mixture of legacy and segwits?).
    // - You want to listen to `bip122_addressesChanged` event (which has the same format as `getAccountAddresses` response)
    //   but we can keep this a todo for later since we probably can manage without it for now.
    // ref. https://docs.reown.com/advanced/multichain/rpc-reference/bitcoin-rpc
    UtxoGetAccountAddresses,
    UtxoSendTransfer,
    UtxoSignPsbt,
    UtxoPersonalSign,
}

impl AsRef<str> for WcRequestMethods {
    fn as_ref(&self) -> &str {
        match self {
            Self::CosmosSignDirect => "cosmos_signDirect",
            Self::CosmosSignAmino => "cosmos_signAmino",
            Self::CosmosGetAccounts => "cosmos_getAccounts",
            Self::EthSignTransaction => "eth_signTransaction",
            Self::EthSendTransaction => "eth_sendTransaction",
            Self::EthPersonalSign => "personal_sign",
            Self::UtxoGetAccountAddresses => "getAccountAddresses",
            Self::UtxoSendTransfer => "sendTransfer",
            Self::UtxoSignPsbt => "signPsbt",
            Self::UtxoPersonalSign => "signMessage",
        }
    }
}
