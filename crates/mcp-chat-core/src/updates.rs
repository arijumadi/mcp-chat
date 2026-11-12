//! Check-only update lookup against the public GitHub releases API.
//!
//! Nothing is downloaded or installed here. A failed check is a normal network
//! error; startup callers can ignore it and manual callers can show it.

use serde::{Deserialize, Serialize};

use crate::{APP_VERSION, CoreError, Result};

const RELEASES_URL: &str = "https://api.github.com/repos/arijumadi/mcp-chat/releases/latest";
const MAX_NOTES_CHARS: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub available: bool,
    pub name: Option<String>,
    pub notes: Option<String>,
    pub url: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub async fn check() -> Result<UpdateInfo> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(format!("Jarvis/{}", APP_VERSION))
        .build()
        .map_err(|error| CoreError::Message(format!("Could not prepare update check: {error}")))?;
    let response = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| {
            CoreError::Message(format!(
                "Could not reach GitHub to check for updates: {error}"
            ))
        })?;
    let status = response.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(CoreError::Message(
            "GitHub rate-limited the update check. Try again later.".into(),
        ));
    }
    if !status.is_success() {
        return Err(CoreError::Message(format!(
            "Update check failed: GitHub returned HTTP {}.",
            status.as_u16()
        )));
    }
    let release: GithubRelease = response.json().await.map_err(|error| {
        CoreError::Message(format!(
            "GitHub returned an invalid release response: {error}"
        ))
    })?;
    if release.draft || release.prerelease {
        return Err(CoreError::Message(
            "The latest GitHub release is not a stable public release.".into(),
        ));
    }
    let latest = release.tag_name.trim().trim_start_matches('v');
    if parse_version(latest).is_none() {
        return Err(CoreError::Message(format!(
            "The latest release tag {:?} is not a valid version.",
            release.tag_name
        )));
    }
    let notes = release
        .body
        .map(|text| crate::util::truncate_chars(&text, MAX_NOTES_CHARS).0);
    Ok(UpdateInfo {
        current_version: APP_VERSION.to_owned(),
        latest_version: latest.to_owned(),
        available: is_newer(latest, APP_VERSION),
        name: release.name,
        notes,
        url: release.html_url,
        published_at: release.published_at,
    })
}

/// Compare dotted numeric versions. A leading `v`, prerelease suffix, and build
/// suffix are ignored. Malformed versions are never considered newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let Some(latest) = parse_version(latest) else {
        return false;
    };
    let Some(current) = parse_version(current) else {
        return false;
    };
    latest > current
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let core = value
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_like_tags() {
        assert!(is_newer("0.3.24", "0.3.23"));
        assert!(is_newer("v0.4.0", "0.3.99"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.3.23", "0.3.23"));
        assert!(!is_newer("0.3.22", "0.3.23"));
        assert!(!is_newer("nope", "0.3.23"));
    }

    #[test]
    fn parses_prerelease_and_build_suffixes() {
        assert_eq!(parse_version("v1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3+build.4"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3.4"), None);
    }
}
