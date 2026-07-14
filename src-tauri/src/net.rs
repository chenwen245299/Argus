//! Shared network guards: URL validation + size-capped response reading.
//!
//! The Rust backend talks to remote hosts with `reqwest` directly, which
//! bypasses the frontend capability's HTTP allow-list. These helpers give the
//! call sites one consistent place to
//!   (a) reject non-HTTP(S) schemes and (for import/metadata fetches) SSRF-prone
//!       private/loopback targets, and
//!   (b) cap how many bytes a response may buffer so a hostile or misbehaving
//!       server cannot exhaust memory (streaming guard, independent of the
//!       advertised `Content-Length`).
//!
//! Kept dependency-free (no `url` crate) with a small hand-rolled parser; the
//! inputs we validate are ordinary absolute URLs, not arbitrary IRIs.

use futures::StreamExt;

/// Split an absolute URL into its lowercased `(scheme, host)`.
/// Returns an error when the string is not `scheme://host…` shaped.
fn scheme_and_host(raw: &str) -> Result<(String, String), String> {
    let trimmed = raw.trim();
    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| "URL must start with a scheme, e.g. https://".to_string())?;
    let scheme = scheme.to_ascii_lowercase();

    // The authority ends at the first '/', '?' or '#'.
    let authority = rest
        .split(|c| c == '/' || c == '?' || c == '#')
        .next()
        .unwrap_or("");
    // Drop any `userinfo@` prefix.
    let host_port = authority.rsplit('@').next().unwrap_or(authority);

    // Separate host from port, handling `[::1]:port` IPv6 literals.
    let host = if let Some(after_bracket) = host_port.strip_prefix('[') {
        after_bracket.split(']').next().unwrap_or(after_bracket)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    Ok((scheme, host.to_ascii_lowercase()))
}

/// True when `host` names a loopback / private / link-local target that an
/// import or metadata fetch should never be pointed at.
fn is_private_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    use std::net::IpAddr;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()      // 127.0.0.0/8
                    || v4.is_private() // 10/8, 172.16/12, 192.168/16
                    || v4.is_link_local() // 169.254/16 (cloud metadata)
                    || v4.is_unspecified() // 0.0.0.0
                    || v4.is_broadcast()
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
                    || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
            }
        };
    }
    false
}

/// Validate a user-configurable AI provider `base_url`. Only the scheme is
/// constrained (http/https) so local model servers — Ollama, LM Studio, etc. on
/// `http://localhost:*` — keep working. This blocks credential-exfiltration
/// schemes such as `file://` and `gopher://`.
pub fn validate_provider_url(raw: &str) -> Result<(), String> {
    let (scheme, host) = scheme_and_host(raw)?;
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Unsupported base URL scheme '{scheme}'. Only http and https are allowed."
        ));
    }
    if host.is_empty() {
        return Err("base URL is missing a host".to_string());
    }
    Ok(())
}

/// Validate a URL that the backend will fetch from the public internet
/// (paper/PDF/metadata imports). Rejects non-http(s) schemes and private /
/// loopback / link-local hosts (SSRF guard). Public hosts — including CDNs and
/// subdomains — are allowed, so legitimate imports are unaffected.
pub fn validate_public_http_url(raw: &str) -> Result<(), String> {
    let (scheme, host) = scheme_and_host(raw)?;
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Unsupported URL scheme '{scheme}'. Only http and https are allowed."
        ));
    }
    if host.is_empty() {
        return Err("URL is missing a host".to_string());
    }
    if is_private_host(&host) {
        return Err(format!(
            "Refusing to fetch a private or loopback address: {host}"
        ));
    }
    Ok(())
}

/// Ensure `raw`'s host equals one of `domains` or is a subdomain of it. Used to
/// keep a fetch pinned to an expected site (e.g. arxiv.org and its subdomains
/// like export.arxiv.org) even when the URL came from user input that merely
/// *contained* the domain as a substring.
pub fn validate_host_suffix(raw: &str, domains: &[&str]) -> Result<(), String> {
    let (scheme, host) = scheme_and_host(raw)?;
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Unsupported URL scheme '{scheme}'. Only http and https are allowed."
        ));
    }
    let ok = domains
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "URL host '{host}' is not one of the expected domains: {domains:?}"
        ))
    }
}

/// Read a response body into memory, refusing to buffer more than `max_bytes`.
/// Checks the advertised `Content-Length` first (fast reject) and then enforces
/// the same ceiling while streaming, so a missing/chunked length cannot be used
/// to slip past the limit.
pub async fn fetch_bytes_capped(
    resp: reqwest::Response,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            return Err(format!(
                "Response too large: {len} bytes exceeds limit of {max_bytes}"
            ));
        }
    }
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Read response body: {e}"))?;
        if buf.len() as u64 + chunk.len() as u64 > max_bytes {
            return Err(format!("Response exceeded size limit of {max_bytes} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// `fetch_bytes_capped`, decoded lossily as UTF-8.
pub async fn fetch_text_capped(resp: reqwest::Response, max_bytes: u64) -> Result<String, String> {
    let bytes = fetch_bytes_capped(resp, max_bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `fetch_bytes_capped`, parsed as JSON.
pub async fn fetch_json_capped<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    max_bytes: u64,
) -> Result<T, String> {
    let bytes = fetch_bytes_capped(resp, max_bytes).await?;
    serde_json::from_slice(&bytes).map_err(|e| format!("Parse JSON response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_url_allows_local_and_https() {
        assert!(validate_provider_url("http://localhost:11434").is_ok());
        assert!(validate_provider_url("http://127.0.0.1:1234/v1").is_ok());
        assert!(validate_provider_url("https://api.openai.com/v1").is_ok());
    }

    #[test]
    fn provider_url_blocks_non_http_schemes() {
        assert!(validate_provider_url("file:///etc/passwd").is_err());
        assert!(validate_provider_url("gopher://evil/").is_err());
        assert!(validate_provider_url("api.openai.com").is_err());
    }

    #[test]
    fn host_suffix_pins_to_domain() {
        assert!(validate_host_suffix("https://arxiv.org/abs/1", &["arxiv.org"]).is_ok());
        assert!(validate_host_suffix("https://export.arxiv.org/pdf/1", &["arxiv.org"]).is_ok());
        assert!(validate_host_suffix("https://evil.com/arxiv.org/abs/1", &["arxiv.org"]).is_err());
        assert!(validate_host_suffix("https://notarxiv.org/abs/1", &["arxiv.org"]).is_err());
    }

    #[test]
    fn public_url_blocks_private_targets() {
        assert!(validate_public_http_url("https://arxiv.org/abs/1234").is_ok());
        assert!(validate_public_http_url("https://ojs.aaai.org/x.pdf").is_ok());
        assert!(validate_public_http_url("file:///etc/passwd.pdf").is_err());
        assert!(validate_public_http_url("http://127.0.0.1/admin.pdf").is_err());
        assert!(validate_public_http_url("http://localhost:8080/x.pdf").is_err());
        assert!(validate_public_http_url("http://169.254.169.254/latest").is_err());
        assert!(validate_public_http_url("http://192.168.1.1/x.pdf").is_err());
        assert!(validate_public_http_url("http://[::1]/x.pdf").is_err());
    }
}
