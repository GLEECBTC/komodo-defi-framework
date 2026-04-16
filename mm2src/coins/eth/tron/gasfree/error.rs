use derive_more::Display;

/// Errors during GasFree activation-time configuration validation.
#[derive(Debug, Display)]
pub enum TronGaslessConfigError {
    #[display(fmt = "GasFree is only supported on TRON chains, got {chain}")]
    UnsupportedChain { chain: String },
    #[display(fmt = "Invalid GasFree service_provider address: {reason}")]
    InvalidServiceProvider { reason: String },
}
