//! Keeping the FREE / 折扣 badges honest after they were first fetched.
//!
//! A model's price is a fact about the provider's catalogue *today*. The badges
//! are written into `ai_providers.json` when the user adds a model and then sit
//! there indefinitely — so a model that stops being free keeps advertising FREE
//! until the user happens to re-fetch the list. That is the one failure mode
//! worth engineering against here: a stale badge does not merely look wrong, it
//! quietly bills someone who believed it.
//!
//! So on launch this re-reads the catalogue in the background and refreshes
//! only the price-derived fields. It is deliberately unhurried: a delay before
//! it starts, one provider at a time, and any failure is dropped silently. The
//! badges being a few minutes late costs nothing; a slow launch costs the user
//! every time.
//!
//! # What it will not touch
//!
//! Only fields that come from the price list. `display_name`, `enabled`,
//! `capabilities` and `provider_order` are the user's, and several of them are
//! routinely hand-edited — overwriting those from the catalogue would undo work
//! nobody asked to have undone.

use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::models::{AiModel, AiProvider};

/// How long to stay out of the way after launch.
///
/// Long enough that the window is up, the library has scanned, and the user is
/// already working. Nothing here is urgent.
const STARTUP_DELAY: Duration = Duration::from_secs(25);

/// Breather between providers, so several configured accounts do not all fire
/// at once.
const BETWEEN_PROVIDERS: Duration = Duration::from_secs(3);

/// Grace period after a model edit, long enough for a burst of saves (adding
/// several models one after another) to settle into one refresh.
const AFTER_EDIT_DELAY: Duration = Duration::from_secs(4);

/// Breather between the per-model discount lookups.
///
/// Promotions live only in `/models/{id}/endpoints`, one request per model, so
/// a user with twenty saved models makes twenty calls. Spacing them keeps this
/// a trickle rather than a burst — nothing here is in a hurry.
const BETWEEN_MODELS: Duration = Duration::from_millis(400);

/// Emitted when a refresh actually changed something, so open windows reload
/// their model lists instead of showing the previous badges until reopened.
const CHANGED_EVENT: &str = "ai-models-refreshed";

/// Whether this provider's catalogue carries prices worth re-reading.
///
/// OpenRouter is the one that publishes per-model pricing, free tiers and
/// time-of-day discounts. Everyone else either has no public `/models` at all
/// (Kimi, Anthropic), or returns bare ids with no prices — and running this
/// against them would spend a request to learn nothing.
fn publishes_prices(provider: &AiProvider) -> bool {
    provider.base_url.to_lowercase().contains("openrouter")
}

/// Copy the price-derived fields from `fresh` onto `saved`.
///
/// Returns whether anything actually changed, so a run that finds nothing new
/// stays silent rather than making every window reload for no reason.
fn apply_offer(saved: &mut AiModel, fresh: &AiModel) -> bool {
    // The bulk list carries schedules but never promotions, so a promotion
    // already on record survives this pass and is refreshed by the per-model
    // lookup afterwards. Copying `fresh.discount_percent` unconditionally would
    // wipe every promotion on every launch and put it back seconds later.
    let promoted = saved.discount_windows.is_empty() && saved.discount_percent.is_some();
    let next_percent = if fresh.discount_windows.is_empty() && promoted {
        saved.discount_percent
    } else {
        fresh.discount_percent
    };

    let changed = saved.is_free != fresh.is_free
        || saved.param_billions != fresh.param_billions
        || saved.discount_percent != next_percent
        || saved.discount_windows != fresh.discount_windows
        || saved.input_price_usd_per_million != fresh.input_price_usd_per_million
        || saved.output_price_usd_per_million != fresh.output_price_usd_per_million;

    saved.is_free = fresh.is_free;
    // Catalogue-derived like the prices, so models saved before this existed
    // pick up their size on the next launch rather than needing a re-fetch.
    saved.param_billions = fresh.param_billions;
    saved.discount_percent = next_percent;
    saved.discount_windows = fresh.discount_windows.clone();
    saved.input_price_usd_per_million = fresh.input_price_usd_per_million;
    saved.output_price_usd_per_million = fresh.output_price_usd_per_million;
    changed
}

/// Drop the badges from a model the catalogue no longer lists.
///
/// Withdrawn models keep working through some providers, so the entry stays —
/// but we can no longer vouch for the price, and "no claim" is the honest state
/// for something we cannot check. The user's own fields are left alone.
fn clear_offer(saved: &mut AiModel) -> bool {
    let changed = saved.is_free || saved.discount_percent.is_some();
    saved.is_free = false;
    saved.discount_percent = None;
    saved.discount_windows.clear();
    changed
}

/// Refresh one provider's saved models against its live catalogue.
///
/// Returns how many entries changed. A failed fetch returns `Ok(0)` having
/// changed nothing: the badges we already have are better than none, and a
/// provider being briefly unreachable is not evidence about its prices.
async fn refresh_provider(root: &str, provider_id: &str) -> usize {
    let settings = crate::ai_manager::read_ai_settings(root);
    let Some(provider) = settings.providers.iter().find(|p| p.id == provider_id) else {
        return 0;
    };
    if provider.models.is_empty() {
        return 0;
    }
    let Some(api_key) = crate::ai_manager::get_api_key(root, provider_id) else {
        return 0;
    };

    let Ok(fresh) = crate::llm::list_models(provider, &api_key).await else {
        return 0;
    };
    if fresh.is_empty() {
        // An empty catalogue is far more likely to be a bad response than every
        // model having been withdrawn at once. Treat it as no information.
        return 0;
    }

    // Re-read rather than reusing the snapshot above: the fetch was a network
    // round-trip, and the user may have edited their models while it was in
    // flight. Whatever they just saved must win.
    let mut settings = crate::ai_manager::read_ai_settings(root);
    let Some(provider) = settings.providers.iter_mut().find(|p| p.id == provider_id) else {
        return 0;
    };

    let by_id: std::collections::HashMap<&str, &AiModel> =
        fresh.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut changed = 0usize;
    for saved in provider.models.iter_mut() {
        let touched = match by_id.get(saved.id.as_str()) {
            Some(current) => apply_offer(saved, current),
            None => clear_offer(saved),
        };
        if touched {
            changed += 1;
        }
    }

    // Persist the cheap pass before going back to the network. The promotion
    // lookups take seconds and end with another read-modify-write, which would
    // read past these edits if they were still only in memory.
    let provider_snapshot = provider.clone();
    if changed > 0 && crate::ai_manager::write_ai_settings(root, &settings).is_err() {
        return 0;
    }

    // Promotions are not in the bulk list, so each saved model needs its own
    // lookup.
    //
    // Only successful lookups are collected. A timeout or a 502 says nothing
    // about whether the promotion is still running, and recording it as "no
    // promotion" would strip the badge off a model that is still discounted —
    // the opposite of the mistake this refresh exists to prevent. What cannot be
    // read is left exactly as it was, to be retried at the next launch.
    let mut promotions: Vec<(String, Option<u32>)> = Vec::new();
    let mut attempted = 0usize;
    let mut unreachable: Option<String> = None;
    for model in &provider_snapshot.models {
        // A schedule already describes when this model is cheaper; a standing
        // promotion would overwrite the windows with "always".
        if !model.discount_windows.is_empty() {
            continue;
        }
        if attempted > 0 {
            tokio::time::sleep(BETWEEN_MODELS).await;
        }
        attempted += 1;
        match crate::llm::fetch_openrouter_discount(
            &provider_snapshot,
            &api_key,
            &model.id,
            model.input_price_usd_per_million,
        )
        .await
        {
            Ok(percent) => promotions.push((model.id.clone(), percent)),
            Err(e) => unreachable = unreachable.or(Some(e)),
        }
    }
    if let Some(first) = unreachable {
        let failed = attempted - promotions.len();
        eprintln!(
            "[offers] {failed} of {attempted} promotion lookups failed; \
             those models keep their current badge. First error — {first}"
        );
    }

    // Re-read once more: the lookups above took a while, and the user may have
    // edited their models in the meantime.
    let mut settings = crate::ai_manager::read_ai_settings(root);
    let Some(provider) = settings.providers.iter_mut().find(|p| p.id == provider_id) else {
        return changed;
    };
    let mut promoted = 0usize;
    for (id, percent) in promotions {
        if let Some(model) = provider.models.iter_mut().find(|m| m.id == id) {
            if model.discount_windows.is_empty() && model.discount_percent != percent {
                model.discount_percent = percent;
                promoted += 1;
            }
        }
    }
    if promoted > 0 && crate::ai_manager::write_ai_settings(root, &settings).is_err() {
        return changed;
    }
    changed + promoted
}

/// Start the launch refresh. Returns immediately.
pub fn spawn(app: &tauri::AppHandle) {
    run_after(app, STARTUP_DELAY);
}

/// Refresh promptly after the user changes their model selection.
///
/// Promotions are not in the bulk list the "获取模型列表" dialog reads — 414
/// models would be 414 extra requests — so a model saved from there arrives
/// with no badge. This fills it in within seconds instead of at the next
/// launch.
pub fn spawn_after_edit(app: &tauri::AppHandle) {
    run_after(app, AFTER_EDIT_DELAY);
}

fn run_after(app: &tauri::AppHandle, delay: Duration) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;

        let Some(root) = current_library(&app) else {
            return;
        };
        let providers: Vec<String> = crate::ai_manager::read_ai_settings(&root)
            .providers
            .iter()
            .filter(|p| p.enabled && publishes_prices(p))
            .map(|p| p.id.clone())
            .collect();

        let mut total = 0usize;
        for (i, id) in providers.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(BETWEEN_PROVIDERS).await;
            }
            total += refresh_provider(&root, id).await;
        }

        if total > 0 {
            let _ = app.emit(CHANGED_EVENT, serde_json::json!({ "updated": total }));
        }
    });
}

fn current_library(app: &tauri::AppHandle) -> Option<String> {
    let state: tauri::State<crate::LibraryRoot> = app.state();
    let guard = state.0.lock().ok()?;
    guard.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, free: bool, discount: Option<u32>) -> AiModel {
        let mut m: AiModel = serde_json::from_value(serde_json::json!({
            "id": id,
            "display_name": id,
        }))
        .unwrap();
        m.is_free = free;
        m.discount_percent = discount;
        if discount.is_some() {
            m.discount_windows = vec![[1000, 100]];
        }
        m
    }

    /// The whole point: a model that stopped being free must stop saying so.
    #[test]
    fn a_withdrawn_free_tier_is_taken_down() {
        let mut saved = model("m", true, None);
        let fresh = model("m", false, None);
        assert!(apply_offer(&mut saved, &fresh));
        assert!(!saved.is_free);
    }

    #[test]
    fn a_new_discount_is_picked_up() {
        let mut saved = model("m", false, None);
        let fresh = model("m", false, Some(50));
        assert!(apply_offer(&mut saved, &fresh));
        assert_eq!(saved.discount_percent, Some(50));
        assert_eq!(saved.discount_windows, vec![[1000, 100]]);
    }

    /// An unchanged catalogue must report no change, or every launch would make
    /// every open window reload its model list for nothing.
    #[test]
    fn an_unchanged_price_reports_no_change() {
        let mut saved = model("m", false, Some(50));
        let fresh = model("m", false, Some(50));
        assert!(!apply_offer(&mut saved, &fresh));
    }

    /// The user's own edits are not the catalogue's to overwrite.
    #[test]
    fn hand_edited_fields_survive_a_refresh() {
        let mut saved = model("m", true, None);
        saved.display_name = "我的模型".into();
        saved.enabled = false;
        saved.capabilities = vec!["tool_calling".into()];
        saved.provider_order = vec!["Together".into()];

        let mut fresh = model("m", false, None);
        fresh.display_name = "Vendor Name".into();
        fresh.enabled = true;
        fresh.capabilities = vec!["vision".into()];

        apply_offer(&mut saved, &fresh);
        assert_eq!(saved.display_name, "我的模型");
        assert!(!saved.enabled, "a disabled model was re-enabled behind the user");
        assert_eq!(saved.capabilities, vec!["tool_calling".to_string()]);
        assert_eq!(saved.provider_order, vec!["Together".to_string()]);
    }

    /// A model gone from the catalogue keeps its entry — it may still work —
    /// but loses a claim we can no longer check.
    #[test]
    fn a_vanished_model_loses_its_badge_but_not_its_entry() {
        let mut saved = model("m", true, Some(50));
        saved.display_name = "kept".into();
        assert!(clear_offer(&mut saved));
        assert!(!saved.is_free);
        assert_eq!(saved.discount_percent, None);
        assert!(saved.discount_windows.is_empty());
        assert_eq!(saved.display_name, "kept");
    }

    /// A standing promotion has no schedule, so it must survive the bulk pass —
    /// which cannot see promotions at all. Overwriting it there would clear
    /// every badge on launch and restore it seconds later, flickering each time.
    #[test]
    fn the_bulk_pass_does_not_clear_a_promotion_it_cannot_see() {
        let mut saved = model("m", false, Some(40));
        saved.discount_windows.clear(); // a promotion has no schedule
        let fresh = model("m", false, None); // the bulk list never carries one
        assert!(!apply_offer(&mut saved, &fresh), "a no-op pass reported a change");
        assert_eq!(saved.discount_percent, Some(40));
    }

    /// But a *scheduled* discount does come from the bulk list, so that one is
    /// the catalogue's to withdraw.
    #[test]
    fn a_withdrawn_schedule_is_taken_down() {
        let mut saved = model("m", false, Some(50));
        saved.discount_windows = vec![[1000, 100]];
        let fresh = model("m", false, None);
        assert!(apply_offer(&mut saved, &fresh));
        assert_eq!(saved.discount_percent, None);
        assert!(saved.discount_windows.is_empty());
    }

    /// Only OpenRouter publishes the prices this reads. Running it elsewhere
    /// spends a request to learn nothing.
    #[test]
    fn only_providers_that_publish_prices_are_polled() {
        let provider = |url: &str| AiProvider {
            id: "p".into(),
            name: "P".into(),
            kind: "openai_compatible".into(),
            base_url: url.into(),
            enabled: true,
            models: vec![],
            created_at: String::new(),
        };
        assert!(publishes_prices(&provider("https://openrouter.ai/api/v1")));
        assert!(!publishes_prices(&provider("https://api.deepseek.com/v1")));
        assert!(!publishes_prices(&provider("http://localhost:11434/v1")));
    }

    /// Launch must not wait on this, and a badge being minutes late costs
    /// nothing next to a slow start.
    #[test]
    fn the_refresh_stays_out_of_the_way_of_launch() {
        assert!(STARTUP_DELAY.as_secs() >= 10);
    }
}
