//! The two halves of a browser profile: who the browser appears to be, and what
//! it remembers.
//!
//! These mirror the API's JSON. The identity is the stealth layer and is
//! optional — a profile without one launches the browser as itself and still
//! carries its cookies and storage.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What gets launched. `Identity::brand` is what it claims to be, which is a
/// different question: a Chrome process can present as Edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    Chrome,
    Firefox,
}

/// What the endpoint handed back by an open session speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Chrome DevTools Protocol, over a websocket.
    Cdp,
    /// WebDriver BiDi. What Firefox returns.
    BiDi,
    /// The browser server named a protocol this crate does not know.
    Unknown,
}

/// A profile as the API returns it.
///
/// Cookie values and the identity's proxy URL are credentials and are not in
/// here: cookies arrive as [`RedactedCookie`], and the identity reports
/// `proxy_set` rather than the URL. Read the values with
/// [`BrowserProfiles::state`](crate::BrowserProfiles::state).
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserProfile {
    pub id: String,
    pub name: String,
    pub browser: BrowserKind,
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub cookies: Vec<RedactedCookie>,
    #[serde(default)]
    pub storage_origins: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Enough to see what a profile is logged into without reading the credential.
#[derive(Debug, Clone, Deserialize)]
pub struct RedactedCookie {
    pub name: String,
    pub domain: String,
    pub path: String,
    #[serde(default)]
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    #[serde(default)]
    pub same_site: String,
    pub value_size: u64,
}

/// What survives between sessions.
///
/// A whole state, never a patch: `PUT .../state` replaces, because the browser
/// hands back its entire cookie jar and a cookie missing from it was deleted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileState {
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    #[serde(default)]
    pub local_storage: Storage,
    #[serde(default)]
    pub session_storage: Storage,
}

/// Origin to key to value. DOM storage is partitioned by origin.
pub type Storage = BTreeMap<String, BTreeMap<String, String>>;

impl ProfileState {
    /// Whether this state would erase a profile if it were saved.
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
            && self.local_storage.values().all(BTreeMap::is_empty)
            && self.session_storage.values().all(BTreeMap::is_empty)
    }
}

/// A cookie with its value. Only [`BrowserProfiles::state`] returns these.
///
/// [`BrowserProfiles::state`]: crate::BrowserProfiles::state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Unix seconds. Absent for a session cookie.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    #[serde(default)]
    pub same_site: String,
}

/// Presentation: user agent inputs, screen metrics, locale, GPU strings, proxy.
///
/// Mirrors the record the browser server applies. Every field is declared
/// rather than passed through as an opaque blob, so a field added upstream is
/// something this crate fails to build over rather than silently drops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    /// Empty or absent means a desktop or laptop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,
    #[serde(default)]
    pub has_battery: bool,
    #[serde(default)]
    pub has_mouse: bool,
    #[serde(default)]
    pub has_touch: bool,
    pub os: Os,
    pub os_version: String,
    pub platform: Platform,
    /// What the browser claims to be, not what it runs on.
    pub brand: Brand,
    /// For example `[124, 0, 6367, 78]`.
    #[serde(default)]
    pub browser_version: Vec<u32>,
    pub screen: Screen,
    pub hardware_concurrency: u32,
    /// `deviceMemory`, in GiB.
    pub memory: u32,
    pub gpu: Gpu,
    pub geo: Geo,
    /// For example `["en-US", "en"]`.
    #[serde(default)]
    pub language: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_count: Option<u32>,
    /// A full URL with credentials.
    ///
    /// Write-only at the API: it is never returned, so an identity read back
    /// from the API always holds `None` here however the profile was created.
    /// Supply one through [`OpenOptions::identity`](crate::OpenOptions) to open
    /// a session behind a proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Whether the stored profile has a proxy. The URL itself is not returned.
    #[serde(default)]
    pub proxy_set: bool,
    /// IANA name. Absent resolves through the proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Os {
    Windows,
    Macintosh,
    Linux,
    Android,
    Ios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Brand {
    Chrome,
    Safari,
    Edge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    /// `navigator.platform` verbatim — `Win32`, `MacIntel`, `Linux x86_64`,
    /// `Linux armv8l`, `iPhone`. Anything else is carried through as itself.
    pub navigator_platform: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Screen {
    pub logical_width: u32,
    pub logical_height: u32,
    pub original_width: u32,
    pub original_height: u32,
    pub density_pixel_ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gpu {
    pub vendor: String,
    pub webgl_renderer: String,
    pub webgl_vendor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::upper_case_acronyms)]
pub enum Geo {
    US,
    UK,
    JP,
    DE,
    FR,
    CA,
    AU,
    IN,
    BR,
    KR,
    IT,
    ES,
    NL,
    PL,
    SE,
    MX,
    SG,
    ZA,
}
