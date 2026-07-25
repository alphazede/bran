//! Provider-neutral boundaries for runtime integrations.

pub mod connected;
pub mod provider;
pub mod sqz;

#[allow(unused_imports)]
pub use provider::{ExternalHostAdapter, SafeAccountReference};

#[allow(unused_imports)]
pub use connected::{
    ConnectedCapabilities, ConnectedCitation, ConnectedClaim, ConnectedCoordinator, ConnectedError,
    ConnectedOutcome, ConnectedPort, ConnectedRequest, ConnectedResponse, ConnectedUsage,
    GroundingReceipt, GroundingStatus,
};

#[allow(unused_imports)]
pub use sqz::{
    is_public_dlp_safe, DlpStatus, ExternalSqzPort, FidelityStatus, SqzAdapter, SqzAdapterConfig,
    SqzError, SqzEvaluation, SqzFailureReason, SqzId, SqzIdentity, SqzPolicy, SqzPort,
    SqzPortError, SqzPortErrorCode, SqzPortOutput, SqzReceipt, SqzStatus, APPROVED_SQZ_SHA256,
    APPROVED_SQZ_SOURCE, APPROVED_SQZ_VERSION,
};
