//! Key-value memory, as a handler sees it.
//!
//! ```no_run
//! use neurun::{Error, Memory};
//! use serde_json::json;
//!
//! # async fn example() -> Result<(), Error> {
//! let memory = Memory::from_env()?;
//! memory.set_with("cursor", &json!({"page": 4}), 3600).await?;
//! let entry = memory.get("cursor").await?;
//! # Ok(())
//! # }
//! ```
//!
//! The organization is never sent. It is prefixed onto every key on the other
//! side, so a key is private to the client that wrote it and there is no key
//! that spells its way out of that.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use crate::connection::{Connection, Token};
use crate::error::{Error, Result};
use crate::memories::memory_client::MemoryClient;
use crate::memories::{DeleteRequest, GetRequest, ListRequest, SetRequest};

type Client = MemoryClient<InterceptedService<Channel, Token>>;

/// One key and what it holds.
///
/// There is no `created_at`: a write replaces, so the only honest timestamp is
/// when the value that is there now was put there.
///
/// The value arrives as a [`Value`], so reading one costs no annotation at the
/// call site. A handler with a shape in mind calls [`parse`](Entry::parse).
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: String,
    pub value: Value,
    /// Unix seconds.
    pub updated_at: i64,
    /// Unix seconds, or `0` for an entry written without a ttl — which keeps it
    /// until something deletes it.
    pub expires_at: i64,
}

impl Entry {
    /// Reads the value as a declared shape.
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.value.clone())
            .map_err(|error| Error::Request(format!("the value could not be read: {error}")))
    }
}

/// A handler's door to its own memory.
///
/// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`, which the worker
/// puts in the environment — the same two [`Browser`](crate::Browser) reads,
/// because it is the same listener and the same credential.
#[derive(Debug, Clone)]
pub struct Memory {
    connection: Connection,
}

impl Memory {
    /// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            connection: Connection::from_env()?,
        })
    }

    pub fn new(address: impl Into<String>, token: impl Into<String>) -> Result<Self> {
        Ok(Self {
            connection: Connection::new(address, token)?,
        })
    }

    /// Where this handle expects the control plane.
    pub fn address(&self) -> &str {
        self.connection.address()
    }

    /// Writes a key, replacing whatever it held, and keeps it until something
    /// deletes it.
    pub async fn set<T: Serialize>(&self, key: impl Into<String>, value: &T) -> Result<Entry> {
        self.set_with(key, value, 0).await
    }

    /// The same write, forgotten after `ttl_seconds`. Zero is no expiry.
    pub async fn set_with<T: Serialize>(
        &self,
        key: impl Into<String>,
        value: &T,
        ttl_seconds: u32,
    ) -> Result<Entry> {
        let encoded = serde_json::to_string(value)
            .map_err(|error| Error::Request(format!("the value is not encodable: {error}")))?;
        let mut client = self.connect().await?;
        let stored = client
            .set(SetRequest {
                key: key.into(),
                value: encoded,
                ttl_seconds,
            })
            .await?
            .into_inner();
        entry(stored)
    }

    /// Reads one key.
    pub async fn get(&self, key: impl Into<String>) -> Result<Entry> {
        let mut client = self.connect().await?;
        let stored = client
            .get(GetRequest { key: key.into() })
            .await?
            .into_inner();
        entry(stored)
    }

    /// Reads the entries whose key starts with `prefix`, by key.
    ///
    /// An empty prefix reads everything this client has. A prefix carrying a
    /// glob character is refused, because it would match keys it does not name.
    pub async fn keys(&self, prefix: impl Into<String>) -> Result<Vec<Entry>> {
        let mut client = self.connect().await?;
        let answer = client
            .list(ListRequest {
                prefix: prefix.into(),
                limit: 0,
            })
            .await?
            .into_inner();
        answer.entries.into_iter().map(entry).collect()
    }

    /// Forgets a key. Forgetting one twice is not an error.
    pub async fn delete(&self, key: impl Into<String>) -> Result<()> {
        let mut client = self.connect().await?;
        client.delete(DeleteRequest { key: key.into() }).await?;
        Ok(())
    }

    async fn connect(&self) -> Result<Client> {
        let (channel, token) = self.connection.open().await?;
        Ok(MemoryClient::with_interceptor(channel, token))
    }
}

fn entry(stored: crate::memories::Entry) -> Result<Entry> {
    let value = serde_json::from_str(&stored.value)
        .map_err(|error| Error::Request(format!("the value could not be read: {error}")))?;
    Ok(Entry {
        key: stored.key,
        value,
        updated_at: stored.updated_at,
        expires_at: stored.expires_at,
    })
}
