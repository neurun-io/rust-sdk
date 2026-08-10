//! The slice of the Neurun API an app talks to while it runs.
//!
//! Three calls, all of them about one browser profile. Creating profiles,
//! projects, apps, deployments and keys is control-plane work that happens
//! before an app exists, and is not in here.

use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::profile::{BrowserProfile, ProfileState};

pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub(crate) struct Api {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl Api {
    pub(crate) fn new(base_url: &str, api_key: &str, timeout: Duration) -> Result<Self> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(Error::configuration(
                "a Neurun API URL is required: pass one, or set NEURUN_URL",
            ));
        }
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(Error::configuration(format!(
                "the Neurun API URL must be http:// or https://, got {base_url}"
            )));
        }
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() {
            return Err(Error::configuration(
                "a Neurun API key is required: pass one, or set NEURUN_API_KEY",
            ));
        }
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            http,
            base_url,
            api_key,
        })
    }

    pub(crate) async fn profile(&self, profile_id: &str) -> Result<BrowserProfile> {
        let request = self.http.get(self.profile_url(profile_id, ""));
        self.send(request).await
    }

    /// Cookie values and storage contents in the clear.
    ///
    /// This is exporting live sessions, so it takes `browser_profiles:write`
    /// rather than `:read`. The values come back in the response body rather
    /// than in a URL, which keeps them out of access logs and history.
    pub(crate) async fn state(&self, profile_id: &str) -> Result<ProfileState> {
        let request = self.http.get(self.profile_url(profile_id, "/state"));
        self.send(request).await
    }

    /// Replaces the stored state, and returns the profile as it now stands.
    ///
    /// A whole-state replace, not a merge: a cookie missing from `state` was
    /// deleted, and merging would resurrect a login the site had already ended.
    pub(crate) async fn save_state(
        &self,
        profile_id: &str,
        state: &ProfileState,
    ) -> Result<BrowserProfile> {
        let request = self
            .http
            .put(self.profile_url(profile_id, "/state"))
            .json(state);
        self.send(request).await
    }

    fn profile_url(&self, profile_id: &str, suffix: &str) -> String {
        format!(
            "{}/v1/browser-profiles/{}{}",
            self.base_url,
            urlencode(profile_id),
            suffix
        )
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        let response = request.bearer_auth(&self.api_key).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(problem(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|error| Error::Api {
            status: status.as_u16(),
            code: "invalid_response".to_string(),
            message: format!("could not read the response body: {error}"),
        })
    }
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Problem,
}

#[derive(Deserialize)]
struct Problem {
    code: String,
    message: String,
}

/// Reads the server's own error code out of the body, and says so plainly when
/// the body is not one — an HTML error page from a proxy in front of the API
/// should not be reported as if the API had answered.
fn problem(status: u16, body: &str) -> Error {
    match serde_json::from_str::<ErrorEnvelope>(body) {
        Ok(envelope) => Error::Api {
            status,
            code: envelope.error.code,
            message: envelope.error.message,
        },
        Err(_) => Error::Api {
            status,
            code: "unknown".to_string(),
            message: truncate(body.trim(), 300),
        },
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let kept: String = value.chars().take(limit).collect();
    format!("{kept}…")
}

/// Path-segment encoding for an identifier that reaches us from an event.
fn urlencode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_and_a_key_are_both_required() {
        assert!(matches!(
            Api::new("", "neu_test_abc.secret", DEFAULT_TIMEOUT),
            Err(Error::Configuration(_))
        ));
        assert!(matches!(
            Api::new("https://api.neurun.io", "  ", DEFAULT_TIMEOUT),
            Err(Error::Configuration(_))
        ));
        assert!(matches!(
            Api::new("api.neurun.io", "k", DEFAULT_TIMEOUT),
            Err(Error::Configuration(_))
        ));
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        let api = Api::new("https://api.neurun.io/", "key", DEFAULT_TIMEOUT).unwrap();
        assert_eq!(
            api.profile_url("bp_1", "/state"),
            "https://api.neurun.io/v1/browser-profiles/bp_1/state"
        );
    }

    #[test]
    fn an_identifier_cannot_walk_out_of_its_path() {
        let api = Api::new("https://api.neurun.io", "key", DEFAULT_TIMEOUT).unwrap();
        assert_eq!(
            api.profile_url("../../v1/api-keys", ""),
            "https://api.neurun.io/v1/browser-profiles/..%2F..%2Fv1%2Fapi-keys"
        );
    }

    #[test]
    fn the_servers_error_code_is_carried_through() {
        let error = problem(404, r#"{"error":{"code":"not_found","message":"no such profile"}}"#);
        match error {
            Error::Api {
                status,
                code,
                message,
            } => {
                assert_eq!(status, 404);
                assert_eq!(code, "not_found");
                assert_eq!(message, "no such profile");
            }
            other => panic!("expected an API error, got {other:?}"),
        }
    }

    #[test]
    fn a_body_that_is_not_a_problem_is_not_dressed_up_as_one() {
        let error = problem(502, "<html>bad gateway</html>");
        match error {
            Error::Api { code, message, .. } => {
                assert_eq!(code, "unknown");
                assert!(message.contains("bad gateway"));
            }
            other => panic!("expected an API error, got {other:?}"),
        }
    }
}
