//! Everything that can go wrong between an app, the API, and the browser server.

use std::fmt;

/// The result every fallible call in this crate returns.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A required environment variable is missing or unusable.
    #[error("{0}")]
    Configuration(String),

    /// The API answered, and said no. `code` is the server's own error code.
    #[error("neurun api: {status} {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },

    /// The API could not be reached, or its answer was not what it claims.
    #[error("neurun api: {0}")]
    Transport(#[from] reqwest::Error),

    /// The browser server on loopback could not be reached.
    #[error("neurun browser: connect to {address}: {source}")]
    BrowserConnect {
        address: String,
        #[source]
        source: tonic::transport::Error,
    },

    /// The browser server answered, and said no.
    #[error("neurun browser: {0}")]
    Browser(#[from] tonic::Status),

    /// A write would have replaced a profile's whole state with nothing.
    ///
    /// `PUT .../state` replaces rather than merges, so an empty body erases the
    /// profile. Saving one is refused because the common way to hold an empty
    /// state is by accident — a Firefox session, which carries none. Erase on
    /// purpose with [`BrowserProfiles::clear_state`].
    ///
    /// [`BrowserProfiles::clear_state`]: crate::BrowserProfiles::clear_state
    #[error(
        "refusing to save an empty state over browser profile {profile_id}: \
         a whole-state replace with nothing erases the profile. \
         Call clear_state to erase it deliberately."
    )]
    EmptyState { profile_id: String },
}

impl Error {
    pub(crate) fn configuration(message: impl fmt::Display) -> Self {
        Error::Configuration(message.to_string())
    }
}
