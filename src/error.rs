//! Everything that can go wrong between a handler and the control plane.

use std::fmt;

/// The result every fallible call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A required environment variable is missing or unusable.
    #[error("{0}")]
    Configuration(String),

    /// The Neurun listener on loopback could not be reached.
    #[error("neurun: connect to {address}: {source}")]
    Connect {
        address: String,
        #[source]
        source: tonic::transport::Error,
    },

    /// The control plane answered, and said no.
    #[error("neurun: {0}")]
    Neurun(#[from] tonic::Status),

    /// The session has already been closed.
    #[error("session {session_id} is already closed")]
    Closed { session_id: String },
}

impl Error {
    pub(crate) fn configuration(message: impl fmt::Display) -> Self {
        Error::Configuration(message.to_string())
    }
}
