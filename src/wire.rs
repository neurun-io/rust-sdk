//! Conversions between the API's JSON and the browser server's protobuf.
//!
//! Both sides describe the same profile, and neither is authoritative over the
//! other: the API stores it, the browser server wears it. This module is the
//! one place the two vocabularies meet.
//!
//! [`identity_to_proto`] builds `proto::Identity` with an **exhaustive struct
//! literal** on purpose. A field added to `browser.proto` upstream lands in the
//! generated struct and stops this function compiling, which is the whole point
//! — drift should be a build failure, not a value quietly dropped on the floor.

use std::collections::{BTreeMap, HashMap};

use crate::profile::{
    Brand, BrowserKind, Cookie, Geo, Gpu, Identity, Os, ProfileState, Protocol, Screen, Storage,
};
use crate::proto;

pub(crate) fn browser_to_proto(browser: BrowserKind) -> proto::BrowserKind {
    match browser {
        BrowserKind::Chrome => proto::BrowserKind::Chrome,
        BrowserKind::Firefox => proto::BrowserKind::Firefox,
    }
}

pub(crate) fn protocol_from_proto(protocol: i32) -> Protocol {
    match proto::Protocol::try_from(protocol) {
        Ok(proto::Protocol::Cdp) => Protocol::Cdp,
        Ok(proto::Protocol::Bidi) => Protocol::BiDi,
        Ok(proto::Protocol::Unspecified) | Err(_) => Protocol::Unknown,
    }
}

pub(crate) fn state_to_proto(state: &ProfileState) -> proto::ProfileState {
    proto::ProfileState {
        cookies: state.cookies.iter().map(cookie_to_proto).collect(),
        local_storage: storage_to_proto(&state.local_storage),
        session_storage: storage_to_proto(&state.session_storage),
    }
}

pub(crate) fn state_from_proto(state: proto::ProfileState) -> ProfileState {
    ProfileState {
        cookies: state.cookies.into_iter().map(cookie_from_proto).collect(),
        local_storage: storage_from_proto(state.local_storage),
        session_storage: storage_from_proto(state.session_storage),
    }
}

fn cookie_to_proto(cookie: &Cookie) -> proto::Cookie {
    proto::Cookie {
        name: cookie.name.clone(),
        value: cookie.value.clone(),
        domain: cookie.domain.clone(),
        path: cookie.path.clone(),
        expires: cookie.expires,
        secure: cookie.secure,
        http_only: cookie.http_only,
        same_site: cookie.same_site.clone(),
    }
}

fn cookie_from_proto(cookie: proto::Cookie) -> Cookie {
    Cookie {
        name: cookie.name,
        value: cookie.value,
        domain: cookie.domain,
        path: cookie.path,
        expires: cookie.expires,
        secure: cookie.secure,
        http_only: cookie.http_only,
        same_site: cookie.same_site,
    }
}

fn storage_to_proto(storage: &Storage) -> Vec<proto::StorageOrigin> {
    storage
        .iter()
        .map(|(origin, entries)| proto::StorageOrigin {
            origin: origin.clone(),
            entries: entries
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<HashMap<_, _>>(),
        })
        .collect()
}

fn storage_from_proto(origins: Vec<proto::StorageOrigin>) -> Storage {
    origins
        .into_iter()
        .map(|origin| (origin.origin, origin.entries.into_iter().collect()))
        .collect::<BTreeMap<_, _>>()
}

/// Builds the browser server's identity from the API's.
///
/// Exhaustive by construction — see the module note.
pub(crate) fn identity_to_proto(identity: &Identity) -> proto::Identity {
    let (navigator_platform, navigator_platform_other) =
        navigator_platform_to_proto(&identity.platform.navigator_platform);
    proto::Identity {
        device_model: identity
            .device_model
            .as_ref()
            .filter(|model| !model.is_empty())
            .cloned(),
        has_battery: identity.has_battery,
        has_mouse: identity.has_mouse,
        has_touch: identity.has_touch,
        os: os_to_proto(identity.os) as i32,
        os_version: identity.os_version.clone(),
        platform: Some(proto::Platform {
            bitness: identity.platform.bitness.clone(),
            architecture: identity.platform.architecture.clone(),
            navigator_platform: navigator_platform as i32,
            navigator_platform_other,
            version: identity.platform.version.clone(),
        }),
        brand: brand_to_proto(identity.brand) as i32,
        browser_version: identity.browser_version.clone(),
        screen: Some(screen_to_proto(identity.screen)),
        hardware_concurrency: identity.hardware_concurrency,
        memory: identity.memory,
        gpu: Some(gpu_to_proto(&identity.gpu)),
        geo: geo_to_proto(identity.geo) as i32,
        language: identity.language.clone(),
        history_count: identity.history_count,
        proxy: identity
            .proxy
            .as_ref()
            .filter(|proxy| !proxy.is_empty())
            .cloned(),
        timezone: identity
            .timezone
            .as_ref()
            .filter(|zone| !zone.is_empty())
            .cloned(),
    }
}

fn screen_to_proto(screen: Screen) -> proto::Screen {
    proto::Screen {
        logical_width: screen.logical_width,
        logical_height: screen.logical_height,
        original_width: screen.original_width,
        original_height: screen.original_height,
        density_pixel_ratio: screen.density_pixel_ratio,
    }
}

fn gpu_to_proto(gpu: &Gpu) -> proto::Gpu {
    proto::Gpu {
        vendor: gpu.vendor.clone(),
        webgl_renderer: gpu.webgl_renderer.clone(),
        webgl_vendor: gpu.webgl_vendor.clone(),
    }
}

fn os_to_proto(os: Os) -> proto::Os {
    match os {
        Os::Windows => proto::Os::Windows,
        Os::Macintosh => proto::Os::Macintosh,
        Os::Linux => proto::Os::Linux,
        Os::Android => proto::Os::Android,
        Os::Ios => proto::Os::Ios,
    }
}

fn brand_to_proto(brand: Brand) -> proto::Brand {
    match brand {
        Brand::Chrome => proto::Brand::Chrome,
        Brand::Safari => proto::Brand::Safari,
        Brand::Edge => proto::Brand::Edge,
    }
}

fn geo_to_proto(geo: Geo) -> proto::Geo {
    match geo {
        Geo::US => proto::Geo::Us,
        Geo::UK => proto::Geo::Uk,
        Geo::JP => proto::Geo::Jp,
        Geo::DE => proto::Geo::De,
        Geo::FR => proto::Geo::Fr,
        Geo::CA => proto::Geo::Ca,
        Geo::AU => proto::Geo::Au,
        Geo::IN => proto::Geo::In,
        Geo::BR => proto::Geo::Br,
        Geo::KR => proto::Geo::Kr,
        Geo::IT => proto::Geo::It,
        Geo::ES => proto::Geo::Es,
        Geo::NL => proto::Geo::Nl,
        Geo::PL => proto::Geo::Pl,
        Geo::SE => proto::Geo::Se,
        Geo::MX => proto::Geo::Mx,
        Geo::SG => proto::Geo::Sg,
        Geo::ZA => proto::Geo::Za,
    }
}

/// `navigator.platform` is a string at the API and an enum on the wire.
///
/// The enum carries the five values the browser server knows; anything else
/// travels beside it as `navigator_platform_other`, so a platform string this
/// crate has never heard of still arrives verbatim rather than as a default.
fn navigator_platform_to_proto(platform: &str) -> (proto::NavigatorPlatform, String) {
    match platform.trim().to_ascii_lowercase().as_str() {
        "win32" => (proto::NavigatorPlatform::Win32, String::new()),
        "macintel" => (proto::NavigatorPlatform::MacIntel, String::new()),
        "linux x86_64" => (proto::NavigatorPlatform::LinuxX8664, String::new()),
        "linux armv8l" => (proto::NavigatorPlatform::LinuxArmV81, String::new()),
        "iphone" => (proto::NavigatorPlatform::Iphone, String::new()),
        _ => (
            proto::NavigatorPlatform::Other,
            platform.trim().to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(origin: &str, key: &str, value: &str) -> Storage {
        BTreeMap::from([(
            origin.to_string(),
            BTreeMap::from([(key.to_string(), value.to_string())]),
        )])
    }

    #[test]
    fn state_survives_a_round_trip() {
        let state = ProfileState {
            cookies: vec![Cookie {
                name: "session".into(),
                value: "opaque".into(),
                domain: ".example.com".into(),
                path: "/".into(),
                expires: Some(1_800_000_000.0),
                secure: true,
                http_only: true,
                same_site: "Lax".into(),
            }],
            local_storage: storage("https://example.com", "theme", "dark"),
            session_storage: storage("https://example.com", "step", "2"),
        };

        let returned = state_from_proto(state_to_proto(&state));

        assert_eq!(returned.cookies.len(), 1);
        assert_eq!(returned.cookies[0].value, "opaque");
        assert_eq!(returned.cookies[0].expires, Some(1_800_000_000.0));
        assert_eq!(returned.local_storage, state.local_storage);
        assert_eq!(returned.session_storage, state.session_storage);
    }

    #[test]
    fn a_session_cookie_keeps_no_expiry() {
        let cookie = proto::Cookie {
            name: "sid".into(),
            value: "x".into(),
            domain: "example.com".into(),
            path: "/".into(),
            expires: None,
            secure: false,
            http_only: false,
            same_site: String::new(),
        };
        assert!(cookie_from_proto(cookie).expires.is_none());
    }

    #[test]
    fn an_unknown_navigator_platform_travels_beside_the_enum() {
        let (platform, other) = navigator_platform_to_proto("FreeBSD amd64");
        assert_eq!(platform, proto::NavigatorPlatform::Other);
        assert_eq!(other, "FreeBSD amd64");

        let (platform, other) = navigator_platform_to_proto("MacIntel");
        assert_eq!(platform, proto::NavigatorPlatform::MacIntel);
        assert!(other.is_empty());
    }

    #[test]
    fn an_unknown_protocol_is_named_rather_than_guessed() {
        assert_eq!(protocol_from_proto(1), Protocol::Cdp);
        assert_eq!(protocol_from_proto(2), Protocol::BiDi);
        assert_eq!(protocol_from_proto(0), Protocol::Unknown);
        assert_eq!(protocol_from_proto(97), Protocol::Unknown);
    }
}
