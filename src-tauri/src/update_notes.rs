//! Read the published release notes between the installed and offered versions.
//! The updater's signed manifest still decides which version can be installed;
//! this read-only GitHub request supplies history for its announcement only.

use semver::{BuildMetadata, Version};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

const RELEASES_URL: &str = "https://api.github.com/repos/chenwen245299/Argus/releases";
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_PAGE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ReleaseNote {
    pub version: String,
    pub body: String,
    pub published_at: Option<String>,
}

fn parse_version(raw: &str) -> Result<Version, String> {
    let raw = raw.trim();
    let mut version = Version::parse(raw.strip_prefix(['v', 'V']).unwrap_or(raw))
        .map_err(|_| format!("Invalid release version: {raw}"))?;
    // Build metadata does not change SemVer precedence or create a new release.
    version.build = BuildMetadata::EMPTY;
    Ok(version)
}

fn select_notes(
    releases: Vec<GitHubRelease>,
    current: &Version,
    target: &Version,
) -> Vec<ReleaseNote> {
    let mut notes = BTreeMap::new();
    for release in releases {
        if release.draft || release.prerelease {
            continue;
        }
        let Ok(version) = parse_version(&release.tag_name) else {
            continue;
        };
        if !version.pre.is_empty() || version <= *current || version > *target {
            continue;
        }
        // GitHub can list out-of-order releases (e.g. a backfilled changelog).
        // Filter and sort by version only after every page has been read.
        notes.entry(version.clone()).or_insert(ReleaseNote {
            version: version.to_string(),
            body: release.body.unwrap_or_default(),
            published_at: release.published_at,
        });
    }
    notes.into_values().rev().collect()
}

fn has_next_page(links: &str) -> bool {
    links.split(',').any(|link| {
        link.split(';')
            .skip(1)
            .any(|part| part.trim() == "rel=\"next\"")
    })
}

async fn read_releases(client: &reqwest::Client, url: &str) -> Result<Vec<GitHubRelease>, String> {
    let mut releases = Vec::new();
    for page in 1..=MAX_PAGES {
        let response = client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .query(&[("per_page", PAGE_SIZE), ("page", page)])
            .send()
            .await
            .map_err(|e| format!("Release history request failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("Release history unavailable: {e}"))?;
        let more = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .is_some_and(has_next_page);
        let bytes = crate::net::fetch_bytes_capped(response, MAX_PAGE_BYTES).await?;
        let batch: Vec<GitHubRelease> =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid release history: {e}"))?;
        releases.extend(batch);
        if !more {
            return Ok(releases);
        }
    }
    // Never silently describe a truncated history as all missed versions.
    Err("Release history exceeds the page limit".into())
}

pub async fn fetch(
    current_version: &str,
    target_version: &str,
) -> Result<Vec<ReleaseNote>, String> {
    let current = parse_version(current_version)?;
    let target = parse_version(target_version)?;
    if target <= current {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::builder()
        .user_agent("Argus-release-notes")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let releases = tokio::time::timeout(
        Duration::from_secs(30),
        read_releases(&client, RELEASES_URL),
    )
    .await
    .map_err(|_| "Release history request timed out".to_string())??;
    Ok(select_notes(releases, &current, &target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn release(tag: &str, body: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.into(),
            body: Some(body.into()),
            published_at: None,
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn six_missed_releases_are_sorted_newest_first_and_bounded_by_versions() {
        let releases = vec![
            release("v0.114.0", "114"),
            release("v0.109.0", "already installed"),
            release("v0.112.0", "112"),
            release("v0.110.0", "110"),
            release("v0.116.0", "future"),
            release("v0.115.0", "115"),
            release("v0.111.0", "111"),
            release("v0.113.0", "113"),
        ];
        let notes = select_notes(
            releases,
            &parse_version("0.109.0").unwrap(),
            &parse_version("0.115.0").unwrap(),
        );
        assert_eq!(
            notes.iter().map(|n| n.version.as_str()).collect::<Vec<_>>(),
            ["0.115.0", "0.114.0", "0.113.0", "0.112.0", "0.111.0", "0.110.0"]
        );
        assert_eq!(notes.last().unwrap().body, "110");
    }

    #[test]
    fn semver_order_excludes_drafts_previews_and_duplicate_builds() {
        let mut draft = release("0.10.2", "draft");
        draft.draft = true;
        let mut preview = release("0.10.1", "preview");
        preview.prerelease = true;
        let notes = select_notes(
            vec![
                draft,
                preview,
                release("v0.10.0-beta.1", "beta"),
                release("nightly", "invalid"),
                release("v0.10.0", "ten"),
                release("0.10.0+build.2", "duplicate"),
                release("v0.9.0", "nine"),
            ],
            &parse_version("v0.9.0+local").unwrap(),
            &parse_version("0.10.2").unwrap(),
        );
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].version, "0.10.0");
        assert_eq!(notes[0].body, "ten");
        assert!(parse_version("bad").is_err());
    }

    #[test]
    fn empty_bodies_and_dates_are_retained_without_inventing_notes() {
        let mut empty = release("v1.1.0", "");
        empty.body = None;
        empty.published_at = Some("2026-10-13T01:00:00Z".into());
        let notes = select_notes(
            vec![empty],
            &parse_version("1.0.0").unwrap(),
            &parse_version("1.1.0").unwrap(),
        );
        assert_eq!(notes[0].body, "");
        assert_eq!(
            notes[0].published_at.as_deref(),
            Some("2026-10-13T01:00:00Z")
        );
    }

    #[tokio::test]
    async fn pagination_reads_later_pages_even_after_an_older_version() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/releases", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for (page, body) in [
                (1, r#"[{"tag_name":"v0.9.0","body":"old"}]"#),
                (2, r#"[{"tag_name":"v0.11.0","body":"backfilled"}]"#),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4096];
                let len = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..len]).contains(&format!("page={page}")));
                let link = if page == 1 {
                    "Link: <https://example.invalid/ignored>; rel=\"next\"\r\n"
                } else {
                    ""
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n{link}Connection: close\r\n\r\n{body}", body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let releases = read_releases(&client, &url).await.unwrap();
        server.await.unwrap();
        let notes = select_notes(
            releases,
            &parse_version("0.10.0").unwrap(),
            &parse_version("0.12.0").unwrap(),
        );
        assert_eq!(notes[0].body, "backfilled");
    }
}
