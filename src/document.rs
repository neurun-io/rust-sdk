//! The document store, as a handler sees it.
//!
//! ```no_run
//! use neurun::{Documents, Error};
//! use serde_json::json;
//!
//! # async fn example() -> Result<(), Error> {
//! let mut people = Documents::from_env()?.collection("people");
//! people.insert(&json!({"name": "ada", "age": 36})).await?;
//! let found = people.find(&json!({"age": {"$gte": 30}})).await?;
//! # Ok(())
//! # }
//! ```
//!
//! A collection is implicit. Nothing declares one, so inserting into a name
//! creates it and emptying it removes it — [`Documents::collection`] reaches for
//! a name and does not go anywhere to do it.
//!
//! The organization is never sent. It is resolved from the execution token on
//! the other side, which is what makes a collection name safe to choose freely:
//! there is no name that reaches another client's documents.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use crate::connection::{Connection, Token};
use crate::documents::documents_client::DocumentsClient;
use crate::documents::{
    DeleteRequest, FindRequest, GetRequest, InsertRequest, ListCollectionsRequest, ReplaceRequest,
    UpdateRequest,
};
use crate::error::{Error, Result};

type Client = DocumentsClient<InterceptedService<Channel, Token>>;

/// A handler's door to its own document store.
///
/// Reads `NEURUN_GRPC_ADDRESS` and `NEURUN_EXECUTION_TOKEN`, which the worker
/// puts in the environment — the same two [`Browser`](crate::Browser) reads,
/// because it is the same listener and the same credential.
#[derive(Debug, Clone)]
pub struct Documents {
    connection: Connection,
}

impl Documents {
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

    /// Reaches for a collection by name. Nothing is created until a write.
    pub fn collection(&self, name: impl Into<String>) -> Collection {
        Collection {
            documents: self.clone(),
            name: name.into(),
        }
    }

    /// Names what this organization has stored, and how much is in each.
    pub async fn collections(&self) -> Result<Vec<CollectionInfo>> {
        let mut client = self.connect().await?;
        let answer = client
            .list_collections(ListCollectionsRequest { limit: 0 })
            .await?
            .into_inner();
        Ok(answer
            .collections
            .into_iter()
            .map(|entry| CollectionInfo {
                name: entry.name,
                documents: entry.documents,
            })
            .collect())
    }

    async fn connect(&self) -> Result<Client> {
        let (channel, token) = self.connection.open().await?;
        Ok(DocumentsClient::with_interceptor(channel, token))
    }
}

/// One collection: a name, and how much is in it.
#[derive(Debug, Clone)]
pub struct CollectionInfo {
    pub name: String,
    pub documents: i64,
}

/// One stored JSON object, and when it was written.
///
/// The body arrives as a [`Value`] rather than as a type parameter, so reading
/// one costs no annotation at the call site. A handler that has a shape in mind
/// calls [`parse`](Document::parse) for it.
#[derive(Debug, Clone)]
pub struct Document {
    pub id: String,
    pub collection: String,
    pub data: Value,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

impl Document {
    /// Reads the body as a declared shape.
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.data.clone())
            .map_err(|error| Error::Request(format!("the document could not be read: {error}")))
    }
}

/// One named collection of documents.
#[derive(Debug, Clone)]
pub struct Collection {
    documents: Documents,
    name: String,
}

impl Collection {
    /// The collection this handle names.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Writes a new document and returns it with the id it was given.
    pub async fn insert<T: Serialize>(&mut self, data: &T) -> Result<Document> {
        let mut client = self.documents.connect().await?;
        let stored = client
            .insert(InsertRequest {
                collection: self.name.clone(),
                data: encode(data)?,
            })
            .await?
            .into_inner();
        document(stored)
    }

    /// Reads one document by id.
    pub async fn get(&mut self, document_id: impl Into<String>) -> Result<Document> {
        let mut client = self.documents.connect().await?;
        let stored = client
            .get(GetRequest {
                collection: self.name.clone(),
                document_id: document_id.into(),
            })
            .await?
            .into_inner();
        document(stored)
    }

    /// Reads the documents a filter matches, newest first.
    ///
    /// The grammar is small and closed:
    ///
    /// ```text
    /// {"name": "ada"}                  the field equals the value
    /// {"name": {"$ne": "ada"}}         it does not
    /// {"name": {"$in": ["ada", "g"]}}  it equals one of them
    /// {"age": {"$gte": 30}}            ordered comparison
    /// {"email": {"$exists": true}}     the field is present
    /// ```
    ///
    /// A dot walks into a nested object. An empty filter reads the whole
    /// collection. An operator outside that list is refused rather than
    /// ignored — a filter that quietly drops a clause matches more than was
    /// asked for.
    pub async fn find<T: Serialize>(&mut self, filter: &T) -> Result<Vec<Document>> {
        self.find_with(filter, 0).await
    }

    /// The same read, stopping after `limit` documents. Zero is no limit.
    pub async fn find_with<T: Serialize>(
        &mut self,
        filter: &T,
        limit: u32,
    ) -> Result<Vec<Document>> {
        let mut client = self.documents.connect().await?;
        let answer = client
            .find(FindRequest {
                collection: self.name.clone(),
                filter: encode(filter)?,
                limit,
            })
            .await?
            .into_inner();
        answer.documents.into_iter().map(document).collect()
    }

    /// Swaps the whole body. A field left out of `data` is gone.
    pub async fn replace<T: Serialize>(
        &mut self,
        document_id: impl Into<String>,
        data: &T,
    ) -> Result<Document> {
        let mut client = self.documents.connect().await?;
        let stored = client
            .replace(ReplaceRequest {
                collection: self.name.clone(),
                document_id: document_id.into(),
                data: encode(data)?,
            })
            .await?
            .into_inner();
        document(stored)
    }

    /// Writes the named fields and leaves the rest.
    ///
    /// The merge is shallow, and a JSON `null` removes a field — which is the
    /// only way to delete one without rewriting the document.
    pub async fn update<T: Serialize>(
        &mut self,
        document_id: impl Into<String>,
        data: &T,
    ) -> Result<Document> {
        let mut client = self.documents.connect().await?;
        let stored = client
            .update(UpdateRequest {
                collection: self.name.clone(),
                document_id: document_id.into(),
                data: encode(data)?,
            })
            .await?
            .into_inner();
        document(stored)
    }

    /// Removes one document.
    pub async fn delete(&mut self, document_id: impl Into<String>) -> Result<()> {
        let mut client = self.documents.connect().await?;
        client
            .delete(DeleteRequest {
                collection: self.name.clone(),
                document_id: document_id.into(),
            })
            .await?;
        Ok(())
    }
}

/// The body crosses as encoded JSON rather than as a protobuf `Struct`, because
/// a `Struct` cannot carry an integer that stays an integer, and a client that
/// stored 1 is owed 1 rather than 1.0 when it reads back.
fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| Error::Request(format!("the document is not encodable: {error}")))
}

fn document(stored: crate::documents::Document) -> Result<Document> {
    let data = serde_json::from_str(&stored.data)
        .map_err(|error| Error::Request(format!("the document could not be read: {error}")))?;
    Ok(Document {
        id: stored.id,
        collection: stored.collection,
        data,
        created_at: stored.created_at,
        updated_at: stored.updated_at,
    })
}
