use thiserror::Error;

#[derive(Error, Debug)]
pub enum VertexaError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("signer error: {0}")]
    Signer(String),

    #[error("contract error: {0}")]
    Contract(String),

    #[error("price feed error: {0}")]
    PriceFeed(String),

    #[error("stale data: {0}")]
    StaleData(String),

    #[error("risk check failed: {0}")]
    RiskCheck(String),

    #[error("mev guard abort: {0}")]
    MevAbort(String),

    #[error("no trade decision: {0}")]
    NoDecision(String),

    #[error("transaction failed: {0}")]
    TransactionFailed(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("trade not profitable: ratio {reward_to_cost:.2} below minimum {minimum:.2}")]
    TradeNotProfitable {
        reward_to_cost: f64,
        minimum: f64,
    },

    #[error("simulation failed after {max_retries} retries")]
    SimulationFailed {
        max_retries: u8,
    },
}

impl From<config::ConfigError> for VertexaError {
    fn from(e: config::ConfigError) -> Self {
        VertexaError::Config(e.to_string())
    }
}

impl From<alloy::transports::TransportError> for VertexaError {
    fn from(e: alloy::transports::TransportError) -> Self {
        VertexaError::Provider(e.to_string())
    }
}

impl From<alloy::signers::Error> for VertexaError {
    fn from(e: alloy::signers::Error) -> Self {
        VertexaError::Signer(e.to_string())
    }
}

impl From<eyre::ErrReport> for VertexaError {
    fn from(e: eyre::ErrReport) -> Self {
        VertexaError::Internal(e.to_string())
    }
}

impl From<serde_json::Error> for VertexaError {
    fn from(e: serde_json::Error) -> Self {
        VertexaError::Parse(e.to_string())
    }
}

impl From<reqwest::Error> for VertexaError {
    fn from(e: reqwest::Error) -> Self {
        VertexaError::Network(e.to_string())
    }
}
