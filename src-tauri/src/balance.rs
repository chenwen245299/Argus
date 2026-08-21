//! Account balance lookups for the providers that publish one.
//!
//! Two of the services Argus talks to expose what is left in the account, and
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
//!
//! [`ProviderBalance`] is the shape the UI renders: one remaining figure with a
//! currency, plus whatever breakdown the provider happened to give.
//!
//! References:
//! <https://api-docs.deepseek.com/zh-cn/api/get-user-balance>,
//! <https://openrouter.ai/docs/api-reference/get-credits>

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
    /// OpenRouter.
    pub currency: String,
    /// DeepSeek: the promotional part of `remaining`, which expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted: Option<f64>,
    /// DeepSeek: the paid-for part of `remaining`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topped_up: Option<f64>,
    /// OpenRouter: credits bought to date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_credits: Option<f64>,
    /// OpenRouter: credits spent to date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_usage: Option<f64>,
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
    crate::llm::is_deepseek(provider) || is_openrouter(provider)
}

fn is_openrouter(provider: &AiProvider) -> bool {
    provider.kind == "openrouter" || provider.base_url.to_lowercase().contains("openrouter")
}

/// Look up one provider's balance.
pub async fn fetch(provider: &AiProvider, api_key: &str) -> Result<ProviderBalance, String> {
    if crate::llm::is_deepseek(provider) {
        return fetch_deepseek(provider, api_key).await;
    }
    if is_openrouter(provider) {
        return fetch_openrouter(provider, api_key).await;
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
        is_available: remaining.map_or(true, |r| r > 0.0),
        other_currencies: Vec::new(),
    })
}

/// True for the statuses that mean "this key may not ask this question", which
/// is the signal to fall back rather than to give up.
fn is_auth_error(message: &str) -> bool {
    message.contains("(401)") || message.contains("(403)") || message.contains("error 403")
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
    fn only_the_two_providers_that_publish_one_are_asked() {
        assert!(supports_balance(&provider(
            "openai_compatible",
            "https://api.deepseek.com/v1"
        )));
        assert!(supports_balance(&provider(
            "openrouter",
            "https://openrouter.ai/api/v1"
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
