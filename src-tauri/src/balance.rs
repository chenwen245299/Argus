//! Account balance lookups for the providers that publish one.
//!
//! Three of the services Argus talks to expose what is left in the account, and
//! they disagree about almost everything — the path, the currency, and whether
//! the number reported is what remains or what has been spent:
//!
//!   * DeepSeek: `GET /user/balance` returns one entry per currency, each with
//!     the remaining total already split into granted (promotional) and
//!     topped-up (prepaid) parts.
//!   * OpenRouter: `GET /credits` returns credits *purchased* and credits
//!     *used*, in USD, and the remainder has to be subtracted out. That endpoint
//!     wants a management key, so an ordinary inference key falls back to
//!     `GET /key`, which reports the same figures from the key's point of view.
//!   * MoleAPI: `GET /api/usage/token/` (beside `/v1`, not under it) reports the
//!     *key's* quota — granted, used, remaining — as an integer in the gateway's
//!     own unit, which `/api/status` says how to turn into dollars. A key can
//!     also be uncapped, in which case it has no remaining figure at all and the
//!     money that matters is the *account's*, which only the console's access
//!     token (系统访问令牌) can read, via `GET /api/user/self`. When the user has
//!     supplied one, that is the figure shown.
//!
//! [`ProviderBalance`] is the shape the UI renders: one remaining figure with a
//! currency, plus whatever breakdown the provider happened to give.
//!
//! References:
//! <https://api-docs.deepseek.com/zh-cn/api/get-user-balance>,
//! <https://openrouter.ai/docs/api-reference/get-credits>,
//! <https://docs.moleapi.com/zh-CN/docs/api/management/token-management/usage-token-get>

use serde::{Deserialize, Serialize};

use crate::models::AiProvider;

/// What one provider says is left in the account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalance {
    pub provider_id: String,
    /// What is left to spend, in `currency`.
    pub remaining: f64,
    /// ISO code as the provider reports it — `CNY` for DeepSeek, `USD` for
    /// OpenRouter and MoleAPI.
    pub currency: String,
    /// DeepSeek: the promotional part of `remaining`, which expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted: Option<f64>,
    /// DeepSeek: the paid-for part of `remaining`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topped_up: Option<f64>,
    /// OpenRouter: credits bought to date. MoleAPI: the quota the key was
    /// issued with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_credits: Option<f64>,
    /// OpenRouter / MoleAPI: spent to date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_usage: Option<f64>,
    /// The key has no cap of its own (an uncapped OpenRouter key, a MoleAPI key
    /// issued as 无限额度) and draws on an account it cannot see into.
    /// `remaining` is then meaningless and the UI shows the spend instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unlimited: bool,
    /// MoleAPI, when `remaining` is the account balance: what the key itself
    /// may still spend, if it has a cap of its own. The key stops first when
    /// this is the smaller figure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_remaining: Option<f64>,
    /// MoleAPI: when the key stops working, as Unix seconds. `None` when it
    /// never expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// False once the account can no longer be charged for a call — DeepSeek
    /// says so outright, and an exhausted OpenRouter balance is treated the
    /// same way.
    pub is_available: bool,
    /// The other currencies DeepSeek reported, if the account holds more than
    /// one. Empty for everyone else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_currencies: Vec<CurrencyBalance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyBalance {
    pub currency: String,
    pub remaining: f64,
}

/// Whether this provider publishes a balance at all. Everything else — a local
/// Ollama, a self-hosted gateway, an Anthropic key — has nothing to ask.
pub fn supports_balance(provider: &AiProvider) -> bool {
    crate::llm::is_deepseek(provider)
        || is_openrouter(provider)
        || crate::moleapi::is_moleapi(provider)
}

fn is_openrouter(provider: &AiProvider) -> bool {
    provider.kind == "openrouter" || provider.base_url.to_lowercase().contains("openrouter")
}

/// Look up one provider's balance.
///
/// `access_token` is the provider's account-level secret, when the user has
/// supplied one (MoleAPI's 系统访问令牌). Only MoleAPI reads it; the others
/// have a single key that answers for the account.
pub async fn fetch(
    provider: &AiProvider,
    api_key: &str,
    access_token: Option<&str>,
) -> Result<ProviderBalance, String> {
    if crate::llm::is_deepseek(provider) {
        return fetch_deepseek(provider, api_key).await;
    }
    if is_openrouter(provider) {
        return fetch_openrouter(provider, api_key).await;
    }
    if crate::moleapi::is_moleapi(provider) {
        return fetch_moleapi(provider, api_key, access_token).await;
    }
    Err(format!("{} does not publish an account balance.", provider.name))
}

// ── DeepSeek ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DeepSeekBalance {
    #[serde(default)]
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<DeepSeekBalanceInfo>,
}

#[derive(Deserialize)]
struct DeepSeekBalanceInfo {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

async fn fetch_deepseek(provider: &AiProvider, api_key: &str) -> Result<ProviderBalance, String> {
    let url = format!("{}/user/balance", provider.base_url.trim_end_matches('/'));
    let body: DeepSeekBalance = get_json(&url, api_key).await?;

    // The amounts arrive as strings ("110.00"), so they survive the wire exactly
    // as DeepSeek formatted them; parse to a number for display and arithmetic.
    let mut infos = body.balance_infos.into_iter();
    let primary = infos
        .next()
        .ok_or("DeepSeek returned no balance information.")?;

    Ok(ProviderBalance {
        provider_id: provider.id.clone(),
        remaining: parse_amount(&primary.total_balance),
        currency: normalise_currency(&primary.currency, "CNY"),
        granted: Some(parse_amount(&primary.granted_balance)),
        topped_up: Some(parse_amount(&primary.topped_up_balance)),
        total_credits: None,
        total_usage: None,
        unlimited: false,
        key_remaining: None,
        expires_at: None,
        is_available: body.is_available,
        other_currencies: infos
            .map(|info| CurrencyBalance {
                currency: normalise_currency(&info.currency, "CNY"),
                remaining: parse_amount(&info.total_balance),
            })
            .collect(),
    })
}

fn parse_amount(raw: &str) -> f64 {
    raw.trim().replace(',', "").parse::<f64>().unwrap_or(0.0)
}

fn normalise_currency(raw: &str, fallback: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_uppercase()
    }
}

// ── OpenRouter ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OpenRouterEnvelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct OpenRouterCredits {
    #[serde(default)]
    total_credits: f64,
    #[serde(default)]
    total_usage: f64,
}

#[derive(Deserialize)]
struct OpenRouterKey {
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    limit_remaining: Option<f64>,
    #[serde(default)]
    usage: f64,
}

async fn fetch_openrouter(provider: &AiProvider, api_key: &str) -> Result<ProviderBalance, String> {
    let base = provider.base_url.trim_end_matches('/');

    // `/credits` is the account-wide view, and the one that can state what is
    // left outright. It is gated on a management key, so a plain inference key
    // gets turned away and we ask the key about itself instead.
    match get_json::<OpenRouterEnvelope<OpenRouterCredits>>(&format!("{base}/credits"), api_key)
        .await
    {
        Ok(envelope) => {
            let credits = envelope.data;
            let remaining = credits.total_credits - credits.total_usage;
            Ok(ProviderBalance {
                provider_id: provider.id.clone(),
                remaining,
                currency: "USD".to_string(),
                granted: None,
                topped_up: None,
                total_credits: Some(credits.total_credits),
                total_usage: Some(credits.total_usage),
                unlimited: false,
                key_remaining: None,
                expires_at: None,
                is_available: remaining > 0.0,
                other_currencies: Vec::new(),
            })
        }
        Err(e) if is_auth_error(&e) => fetch_openrouter_key(base, api_key, provider).await,
        Err(e) => Err(e),
    }
}

async fn fetch_openrouter_key(
    base: &str,
    api_key: &str,
    provider: &AiProvider,
) -> Result<ProviderBalance, String> {
    let key: OpenRouterKey = get_json::<OpenRouterEnvelope<OpenRouterKey>>(
        &format!("{base}/key"),
        api_key,
    )
    .await
    .map(|e| e.data)?;

    // An uncapped key reports no remaining figure at all. Rather than invent
    // one, report zero remaining and mark it unavailable so the UI shows the
    // spend instead of a confidently wrong balance.
    let remaining = key.limit_remaining.or_else(|| key.limit.map(|l| l - key.usage));
    Ok(ProviderBalance {
        provider_id: provider.id.clone(),
        remaining: remaining.unwrap_or(0.0),
        currency: "USD".to_string(),
        granted: None,
        topped_up: None,
        total_credits: key.limit,
        total_usage: Some(key.usage),
        // An uncapped key has no ceiling to run out of, so it counts as usable.
        unlimited: remaining.is_none(),
        key_remaining: None,
        expires_at: None,
        is_available: remaining.map_or(true, |r| r > 0.0),
        other_currencies: Vec::new(),
    })
}

/// True for the statuses that mean "this key may not ask this question", which
/// is the signal to fall back rather than to give up.
fn is_auth_error(message: &str) -> bool {
    message.contains("(401)") || message.contains("(403)") || message.contains("error 403")
}

// ── MoleAPI ──────────────────────────────────────────────────────────────────

async fn fetch_moleapi(
    provider: &AiProvider,
    api_key: &str,
    access_token: Option<&str>,
) -> Result<ProviderBalance, String> {
    // Three independent reads: the key's quota, the account's balance (only
    // with an access token), and the site's quota-per-dollar. The last is
    // public and merely a conversion factor, so it never fails the lookup — a
    // default stands in when it cannot be read.
    let account = async {
        match access_token {
            Some(token) => crate::moleapi::fetch_account(provider, token).await.map(Some),
            None => Ok(None),
        }
    };
    let (usage, account, per_unit) = tokio::join!(
        crate::moleapi::fetch_token_usage(provider, api_key),
        account,
        crate::moleapi::fetch_quota_per_unit(provider),
    );
    let usage = usage?;
    // A token the user went to the trouble of pasting and that does not work
    // is something to tell them, not to quietly fall back from: the key-only
    // view would show "不限额" and hide the misconfiguration.
    let account = account?;
    Ok(moleapi_balance(provider, &usage, account.as_ref(), per_unit, now_unix()))
}

/// Turn the gateway's quota figures into money. Split from the fetch so the
/// arithmetic and the availability rule are testable against the response
/// shapes the gateway actually produces.
///
/// With an account in hand, `remaining` is the account balance — the money
/// that actually runs out — and the key's own cap, if it has one, rides along
/// as `key_remaining`. Without one, only the key's view is available.
fn moleapi_balance(
    provider: &AiProvider,
    usage: &crate::moleapi::TokenUsage,
    account: Option<&crate::moleapi::AccountInfo>,
    quota_per_unit: f64,
    now_unix: i64,
) -> ProviderBalance {
    let dollars = |quota: f64| quota / quota_per_unit;
    let key_remaining = (!usage.unlimited_quota).then(|| dollars(usage.total_available));
    let expires_at = (usage.expires_at > 0).then_some(usage.expires_at);
    let expired = expires_at.is_some_and(|t| t <= now_unix);
    // The key keeps working while it has quota (or no cap of its own) and has
    // not expired; with an account in view, the account must have money too.
    let key_usable = !expired && key_remaining.map_or(true, |r| r > 0.0);

    match account {
        Some(acct) => {
            let remaining = dollars(acct.quota);
            ProviderBalance {
                provider_id: provider.id.clone(),
                remaining,
                currency: "USD".to_string(),
                granted: None,
                topped_up: None,
                total_credits: None,
                total_usage: Some(dollars(acct.used_quota)),
                unlimited: false,
                key_remaining,
                expires_at,
                is_available: key_usable && remaining > 0.0,
                other_currencies: Vec::new(),
            }
        }
        None => ProviderBalance {
            provider_id: provider.id.clone(),
            // An uncapped key has no remaining figure; report zero and say so
            // via `unlimited` rather than invent one.
            remaining: key_remaining.unwrap_or(0.0),
            currency: "USD".to_string(),
            granted: None,
            topped_up: None,
            total_credits: (!usage.unlimited_quota).then(|| dollars(usage.total_granted)),
            total_usage: Some(dollars(usage.total_used)),
            unlimited: usage.unlimited_quota,
            key_remaining: None,
            expires_at,
            is_available: key_usable,
            other_currencies: Vec::new(),
        },
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Shared ───────────────────────────────────────────────────────────────────

async fn get_json<T: serde::de::DeserializeOwned>(url: &str, api_key: &str) -> Result<T, String> {
    let resp = crate::llm::build_client()?
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = crate::net::fetch_text_capped(resp, 256 * 1024).await?;
    if status >= 400 {
        return Err(crate::llm::friendly_error(status, &text));
    }
    serde_json::from_str(&text).map_err(|e| format!("Unexpected balance response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(kind: &str, base_url: &str) -> AiProvider {
        AiProvider {
            id: "p1".into(),
            name: "P".into(),
            kind: kind.into(),
            base_url: base_url.into(),
            enabled: true,
            models: vec![],
            server_tools: Default::default(),
        created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn only_the_providers_that_publish_one_are_asked() {
        assert!(supports_balance(&provider(
            "openai_compatible",
            "https://api.deepseek.com/v1"
        )));
        assert!(supports_balance(&provider(
            "openrouter",
            "https://openrouter.ai/api/v1"
        )));
        assert!(supports_balance(&provider("moleapi", "https://api.moleapi.com/v1")));
        assert!(supports_balance(&provider(
            "openai_compatible",
            "https://api.moleapi.com/v1"
        )));
        assert!(!supports_balance(&provider("ollama", "http://localhost:11434")));
        assert!(!supports_balance(&provider(
            "anthropic",
            "https://api.anthropic.com/v1"
        )));
    }

    #[test]
    fn deepseek_amounts_arrive_as_strings() {
        assert_eq!(parse_amount("110.00"), 110.0);
        assert_eq!(parse_amount(" 1,234.50 "), 1234.5);
        assert_eq!(parse_amount("nonsense"), 0.0);
    }

    #[test]
    fn a_missing_currency_falls_back_rather_than_showing_blank() {
        assert_eq!(normalise_currency("cny", "USD"), "CNY");
        assert_eq!(normalise_currency("  ", "CNY"), "CNY");
    }

    fn usage(
        available: f64,
        used: f64,
        unlimited: bool,
        expires_at: i64,
    ) -> crate::moleapi::TokenUsage {
        crate::moleapi::TokenUsage {
            total_granted: available + used,
            total_used: used,
            total_available: available,
            unlimited_quota: unlimited,
            expires_at,
        }
    }

    /// 500 000 quota to the dollar, as MoleAPI runs it.
    #[test]
    fn moleapi_quota_is_converted_at_the_site_rate() {
        let p = provider("moleapi", "https://api.moleapi.com/v1");
        let b = moleapi_balance(&p, &usage(3_765_433.0, 1_234_567.0, false, 0), None, 500_000.0, 1_000);
        assert!((b.remaining - 7.530866).abs() < 1e-6);
        assert!((b.total_usage.unwrap() - 2.469134).abs() < 1e-6);
        assert!((b.total_credits.unwrap() - 10.0).abs() < 1e-6);
        assert_eq!(b.currency, "USD");
        assert!(!b.unlimited);
        assert_eq!(b.expires_at, None);
        assert!(b.is_available);
    }

    /// An uncapped key reports no remaining figure rather than a zero that
    /// reads as "empty".
    #[test]
    fn moleapi_uncapped_key_is_usable_with_no_remaining_figure() {
        let p = provider("moleapi", "https://api.moleapi.com/v1");
        let b = moleapi_balance(&p, &usage(0.0, 900_000.0, true, 0), None, 500_000.0, 1_000);
        assert!(b.unlimited);
        assert_eq!(b.remaining, 0.0);
        assert_eq!(b.total_credits, None);
        assert!((b.total_usage.unwrap() - 1.8).abs() < 1e-9);
        assert!(b.is_available);
    }

    #[test]
    fn moleapi_exhausted_or_expired_keys_are_flagged() {
        let p = provider("moleapi", "https://api.moleapi.com/v1");
        assert!(
            !moleapi_balance(&p, &usage(0.0, 500_000.0, false, 0), None, 500_000.0, 1_000).is_available
        );
        // Expired yesterday.
        let b = moleapi_balance(&p, &usage(500_000.0, 0.0, false, 999), None, 500_000.0, 1_000);
        assert!(!b.is_available);
        assert_eq!(b.expires_at, Some(999));
        // Expires tomorrow: still fine, and the date is passed along.
        let b = moleapi_balance(&p, &usage(500_000.0, 0.0, false, 2_000), None, 500_000.0, 1_000);
        assert!(b.is_available);
        assert_eq!(b.expires_at, Some(2_000));
    }

    fn account(quota: f64, used: f64) -> crate::moleapi::AccountInfo {
        crate::moleapi::AccountInfo { quota, used_quota: used }
    }

    /// With the access token, an uncapped key finally has a number: the
    /// account's. `unlimited` goes away, since there is now a balance to show.
    #[test]
    fn moleapi_account_balance_replaces_the_uncapped_placeholder() {
        let p = provider("moleapi", "https://api.moleapi.com/v1");
        let b = moleapi_balance(
            &p,
            &usage(0.0, 900_000.0, true, 0),
            Some(&account(12_345_678.0, 4_000_000.0)),
            500_000.0,
            1_000,
        );
        assert!(!b.unlimited);
        assert!((b.remaining - 24.691356).abs() < 1e-6);
        assert!((b.total_usage.unwrap() - 8.0).abs() < 1e-9);
        assert_eq!(b.key_remaining, None);
        assert_eq!(b.total_credits, None);
        assert!(b.is_available);
    }

    /// A capped key under a funded account: the account figure leads, the
    /// key's own remainder rides along, and whichever is empty stops the key.
    #[test]
    fn moleapi_capped_key_keeps_its_own_remainder_beside_the_account() {
        let p = provider("moleapi", "https://api.moleapi.com/v1");
        let b = moleapi_balance(
            &p,
            &usage(1_000_000.0, 0.0, false, 0),
            Some(&account(50_000_000.0, 0.0)),
            500_000.0,
            1_000,
        );
        assert!((b.remaining - 100.0).abs() < 1e-9);
        assert!((b.key_remaining.unwrap() - 2.0).abs() < 1e-9);
        assert!(b.is_available);

        // Key drained, account still funded: the key is what fails.
        let b = moleapi_balance(
            &p,
            &usage(0.0, 1_000_000.0, false, 0),
            Some(&account(50_000_000.0, 0.0)),
            500_000.0,
            1_000,
        );
        assert!(!b.is_available);

        // Account drained under an uncapped key: the account is what fails.
        let b = moleapi_balance(
            &p,
            &usage(0.0, 0.0, true, 0),
            Some(&account(0.0, 9_000_000.0)),
            500_000.0,
            1_000,
        );
        assert!(!b.is_available);
        assert_eq!(b.remaining, 0.0);
    }

    /// Only an auth refusal should send OpenRouter down the `/key` path — a
    /// network blip or a 500 must surface, not be papered over.
    #[test]
    fn only_auth_failures_trigger_the_key_fallback() {
        assert!(is_auth_error(&crate::llm::friendly_error(403, "management key required")));
        assert!(is_auth_error(&crate::llm::friendly_error(401, "bad key")));
        assert!(!is_auth_error(&crate::llm::friendly_error(500, "oops")));
        assert!(!is_auth_error("Network error: timed out"));
    }
}
