//! The mainland-China public-holiday calendar, kept current in the background.
//!
//! DeepSeek prices its peak window only on Monday–Friday that are not a Chinese
//! statutory holiday (see `src/utils/modelPricing.ts`, `describePeakPeriod`).
//! Which weekdays those are is not computable: the State Council publishes the
//! next year's arrangement each November, with 调休 bridge days (2026-10-05..07
//! are a Monday to Wednesday). The frontend ships the 2025 and 2026
//! arrangements, but without this module the app would be wrong again every
//! January until somebody remembered to release a new table.
//!
//! So: the community dataset NateScarlet/holiday-cn is re-read in the background
//! — one JSON file per year, kept in step with the State Council notice — and
//! cached in the app-data directory. The frontend asks for the cache
//! (`get_cn_holidays`, no network, instant) when a window starts and again when
//! the `cn-holidays-updated` event says the data changed; a year the cache
//! covers replaces the frontend's embedded one.
//!
//! It is deliberately unhurried and silent, in the style of `offer_sync`: a delay
//! after launch, a failed fetch is dropped without a word (the embedded table and
//! the statutory fallback still answer), and the network is touched at most once
//! every [`REFRESH_AFTER_SECS`] — less than that while the cache is fresh.
//!
//! # What it trusts
//!
//! Nothing it did not check. A response is accepted only if it is small, is JSON
//! of the expected shape, names the year that was asked for, every date is a
//! real calendar date inside that year, no date repeats, and the year *looks like
//! a real arrangement* (see [`validate_days`]): the statutory days
//! ([`STATUTORY_OFF_DAYS`]) are days off, and the numbers of days off and of
//! weekday days off sit in the range every published year has ever had
//! ([`MIN_OFF_DAYS`], [`MAX_OFF_DAYS`], [`MAX_WEEKDAY_OFF_DAYS`]). A year that
//! passes replaces the frontend's embedded table outright, so "well-formed" alone
//! is not enough: a bad or partial upstream publish would otherwise halve the
//! estimated cost of ordinary weekdays. The cache file is re-validated on every
//! read, with the same rules, because a file in the app-data directory can be
//! edited, truncated or left by an older build. Only what the frontend needs is
//! kept: date, name, and whether it is a day off (`false` = a make-up working
//! day).
//!
//! No API key is involved. The two mirrors are pinned by host
//! (`net::validate_host_suffix`), and so is every redirect and the URL a response
//! finally came from ([`Pin`]); responses are size-capped
//! (`net::fetch_bytes_capped`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

/// Emitted when a refresh changed the cached calendar, so open windows re-read it.
pub const CHANGED_EVENT: &str = "cn-holidays-updated";

const CACHE_FILE: &str = "cn_holidays.json";
const CACHE_VERSION: u32 = 1;

/// Stay out of the way of launch: the window is up and the library has scanned.
const STARTUP_DELAY: Duration = Duration::from_secs(45);
/// How often a running app looks at whether a refresh is due. Cheap — a local
/// file read — so it only needs to be short enough to notice a new year without
/// a restart.
const RECHECK_EVERY: Duration = Duration::from_secs(12 * 3600);
/// The most often the network is asked: the arrangement changes once a year, and
/// rarely in between.
const REFRESH_AFTER_SECS: i64 = 3 * 24 * 3600;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const BETWEEN_REQUESTS: Duration = Duration::from_secs(2);

/// A year's file is about 5 KB. Anything near this is not a holiday calendar.
const MAX_BODY_BYTES: u64 = 64 * 1024;
/// Entries in one year. Real years have 29–39 (days off plus make-up days).
const MAX_DAYS: usize = 80;
/// Days off in a full year's arrangement. The statutory count is 13 since the
/// 2024-11 revision; the real arrangements, weekends and 调休 included, ran 23 to
/// 33 over 2016–2026 (2025 had 28, 2026 has 33). A file with fewer than this is
/// truncated or not an arrangement, one with more than [`MAX_OFF_DAYS`] is not
/// either. Weekends alone would be 104, which is how an "every day is off" file is
/// told apart.
const MIN_OFF_DAYS: usize = 13;
const MAX_OFF_DAYS: usize = 45;
/// Days off that fall on a Monday–Friday — the only ones that change a price. This is
/// what a poisoned file would inflate (a weekend day marked off changes nothing), and
/// real years have 16–19 (2025: 18, 2026: 19), so 25 leaves room for a longer year
/// without letting 40 ordinary weekdays through.
const MAX_WEEKDAY_OFF_DAYS: usize = 25;
/// `(month, day)` of the days off that no arrangement has ever changed: New Year's
/// Day, Labour Day and the first three days of National Day (checked against every
/// published year 2016–2026). A year missing one of them is not the State
/// Council's arrangement.
const STATUTORY_OFF_DAYS: [(u32, u32); 5] = [(1, 1), (5, 1), (10, 1), (10, 2), (10, 3)];
const MAX_NAME_CHARS: usize = 40;
/// Years behind the current one that are kept, so usage recorded last year is
/// still priced by last year's calendar.
const KEEP_PAST_YEARS: i32 = 2;

/// Tried in order. jsDelivr first: raw.githubusercontent.com is unreliable from
/// mainland China. `{year}` is replaced by the year.
const MIRRORS: [&str; 2] = [
    "https://cdn.jsdelivr.net/gh/NateScarlet/holiday-cn@master/{year}.json",
    "https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/{year}.json",
];
const MIRROR_HOSTS: [&str; 2] = ["cdn.jsdelivr.net", "raw.githubusercontent.com"];

// ── Data ────────────────────────────────────────────────────────────────────

/// One day of an arrangement, in the shape holiday-cn (and the frontend) use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HolidayDay {
    /// `YYYY-MM-DD`, a Beijing calendar date.
    pub date: String,
    pub name: String,
    /// `true`: a day off. `false`: a make-up working day (调休上班).
    pub is_off_day: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearCalendar {
    pub year: i32,
    pub days: Vec<HolidayDay>,
}

/// What lives in the cache file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HolidayCache {
    pub version: u32,
    /// Unix seconds of the last check that got an answer from a mirror.
    pub checked_at: i64,
    pub years: Vec<YearCalendar>,
}

/// What `get_cn_holidays` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HolidaySnapshot {
    pub checked_at: i64,
    pub years: Vec<YearCalendar>,
}

// ── Validation ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RawYear {
    year: i64,
    days: Vec<RawDay>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDay {
    date: String,
    name: String,
    is_off_day: bool,
}

/// Check one year's days and return them normalized (trimmed names, sorted by
/// date). `Ok(None)` is an empty list — a year whose arrangement is not
/// published yet (holiday-cn creates `2027.json` early, with `"days": []`).
///
/// Beyond the shape of each entry, the year as a whole must look like an
/// arrangement: [`STATUTORY_OFF_DAYS`] are all days off, and the days off number
/// between [`MIN_OFF_DAYS`] and [`MAX_OFF_DAYS`], of which at most
/// [`MAX_WEEKDAY_OFF_DAYS`] fall on a weekday.
fn validate_days(year: i32, days: &[HolidayDay]) -> Result<Option<Vec<HolidayDay>>, String> {
    if days.is_empty() {
        return Ok(None);
    }
    if days.len() > MAX_DAYS {
        return Err(format!("{year}: {} entries, expected at most {MAX_DAYS}", days.len()));
    }
    let mut out: Vec<HolidayDay> = Vec::with_capacity(days.len());
    let mut seen = std::collections::HashSet::new();
    let mut weekday_off = 0usize;
    for d in days {
        // chrono accepts `2026-1-1` for `%Y-%m-%d`; the round trip insists on the
        // canonical form, which is also what the frontend looks dates up by.
        let parsed = chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
            .map_err(|_| format!("{year}: bad date {:?}", d.date))?;
        if parsed.format("%Y-%m-%d").to_string() != d.date {
            return Err(format!("{year}: date {:?} is not YYYY-MM-DD", d.date));
        }
        if parsed.year() != year {
            return Err(format!("{year}: date {} belongs to another year", d.date));
        }
        if !seen.insert(d.date.clone()) {
            return Err(format!("{year}: date {} appears twice", d.date));
        }
        let name = d.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS || name.chars().any(|c| c.is_control()) {
            return Err(format!("{year}: bad holiday name for {}", d.date));
        }
        if d.is_off_day && parsed.weekday().number_from_monday() <= 5 {
            weekday_off += 1;
        }
        out.push(HolidayDay { date: d.date.clone(), name: name.to_string(), is_off_day: d.is_off_day });
    }
    let off = out.iter().filter(|d| d.is_off_day).count();
    if !(MIN_OFF_DAYS..=MAX_OFF_DAYS).contains(&off) {
        return Err(format!("{year}: {off} days off is not a plausible year (expected {MIN_OFF_DAYS}–{MAX_OFF_DAYS})"));
    }
    if weekday_off > MAX_WEEKDAY_OFF_DAYS {
        return Err(format!("{year}: {weekday_off} weekdays off is not a plausible year (expected at most {MAX_WEEKDAY_OFF_DAYS})"));
    }
    for (month, day) in STATUTORY_OFF_DAYS {
        let date = format!("{year:04}-{month:02}-{day:02}");
        if !out.iter().any(|d| d.date == date && d.is_off_day) {
            return Err(format!("{year}: {date} is a statutory holiday but is not a day off"));
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(Some(out))
}

/// Parse and strictly validate one downloaded year file.
///
/// `Ok(None)`: well-formed but empty — not published yet. `Err`: reject it.
pub fn parse_year(body: &[u8], expected_year: i32) -> Result<Option<YearCalendar>, String> {
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(format!("response of {} bytes is too large", body.len()));
    }
    let raw: RawYear = serde_json::from_slice(body).map_err(|e| format!("not a holiday calendar: {e}"))?;
    if raw.year != i64::from(expected_year) {
        return Err(format!("asked for {expected_year}, got {}", raw.year));
    }
    let days: Vec<HolidayDay> = raw
        .days
        .into_iter()
        .map(|d| HolidayDay { date: d.date, name: d.name, is_off_day: d.is_off_day })
        .collect();
    Ok(validate_days(expected_year, &days)?.map(|days| YearCalendar { year: expected_year, days }))
}

// ── Time ────────────────────────────────────────────────────────────────────

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The calendar year in Beijing (UTC+8, no DST) at a Unix time.
pub fn beijing_year(unix_secs: i64) -> i32 {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_secs + 8 * 3600, 0)
        .map(|t| t.year())
        .unwrap_or(1970)
}

// ── Cache ───────────────────────────────────────────────────────────────────

/// Machine-local app data, beside `settings.json` — never inside the user's
/// library folder, which is synced between machines and has no business holding
/// a rebuildable cache.
fn cache_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|e| format!("app_local_data_dir: {e}"))?;
    Ok(dir.join(CACHE_FILE))
}

/// Read the cache. Anything wrong — missing file, garbage, another version — is
/// "no cache", and a year that no longer validates is dropped on its own.
pub fn read_cache(path: &Path) -> HolidayCache {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HolidayCache::default();
    };
    let Ok(mut cache) = serde_json::from_str::<HolidayCache>(&text) else {
        return HolidayCache::default();
    };
    if cache.version != CACHE_VERSION {
        return HolidayCache::default();
    }
    let mut years = Vec::new();
    for y in std::mem::take(&mut cache.years) {
        if let Ok(Some(days)) = validate_days(y.year, &y.days) {
            if !years.iter().any(|k: &YearCalendar| k.year == y.year) {
                years.push(YearCalendar { year: y.year, days });
            }
        }
    }
    years.sort_by_key(|y| y.year);
    cache.years = years;
    cache
}

fn write_cache(path: &Path, cache: &HolidayCache) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string(cache).map_err(|e| format!("Serialize holidays cache: {e}"))?;
    crate::fsutil::atomic_write_str(path, &text)
}

/// Whether the network should be asked now.
///
/// Always when the current year is not cached at all; otherwise only when the
/// last successful check is [`REFRESH_AFTER_SECS`] old. A `checked_at` from the
/// far future (the clock was wrong when it was written) counts as stale rather
/// than silencing refreshes for years.
pub fn needs_refresh(cache: &HolidayCache, now: i64) -> bool {
    let year = beijing_year(now);
    if !cache.years.iter().any(|y| y.year == year) {
        return true;
    }
    let age = now - cache.checked_at;
    !(0..REFRESH_AFTER_SECS).contains(&age)
}

/// Fold freshly fetched years into the cache. Returns whether the calendar
/// *content* changed (a bare `checked_at` bump is not a change — windows would
/// reload for nothing).
///
/// A fetched year replaces the cached one, and a cached year that does not pass
/// today's rules is dropped even if nothing replaced it (a cache written by an older
/// build, with looser ones, can hold one); [`years_to_fetch`] then asks for it again.
///
/// `answered`: at least one mirror gave a usable answer, including "not
/// published yet", so the check counts as done.
pub fn merge(cache: &mut HolidayCache, fetched: Vec<YearCalendar>, now: i64, answered: bool) -> bool {
    let before = cache.years.clone();
    for cal in fetched {
        match cache.years.iter_mut().find(|y| y.year == cal.year) {
            Some(slot) => *slot = cal,
            None => cache.years.push(cal),
        }
    }
    let oldest = beijing_year(now) - KEEP_PAST_YEARS;
    cache.years.retain(|y| y.year >= oldest && matches!(validate_days(y.year, &y.days), Ok(Some(_))));
    cache.years.sort_by_key(|y| y.year);
    cache.version = CACHE_VERSION;
    if answered {
        cache.checked_at = now;
    }
    cache.years != before
}

// ── Fetching ────────────────────────────────────────────────────────────────

enum Fetched {
    Calendar(YearCalendar),
    /// Every mirror that answered said there is nothing for this year yet.
    Unpublished,
    Failed(String),
}

fn mirror_urls(year: i32) -> Vec<String> {
    MIRRORS.iter().map(|m| m.replace("{year}", &year.to_string())).collect()
}

/// Redirects followed before giving up. A mirror needs none; one hop is a CDN
/// moving a file.
const MAX_REDIRECTS: usize = 3;

/// Where a fetch may go, and where it may end up. `reqwest` follows redirects to
/// any host by default, so the pin on the first URL alone says nothing about where
/// the bytes finally come from.
#[derive(Clone, Copy)]
struct Pin {
    hosts: &'static [&'static str],
    /// Refuse `http://` (a downgrade the host check alone would let through).
    https_only: bool,
}

/// The two mirrors, over https only.
const MIRROR_PIN: Pin = Pin { hosts: &MIRROR_HOSTS, https_only: true };

impl Pin {
    fn allows(&self, url: &reqwest::Url) -> bool {
        (!self.https_only || url.scheme() == "https") && crate::net::validate_host_suffix(url.as_str(), self.hosts).is_ok()
    }

    /// This module's own client, not the shared `llm::build_client` one, whose default
    /// policy follows up to ten redirects anywhere. Every hop is held to the pin
    /// again; one that leaves it fails the request, so the next mirror is asked.
    fn client_builder(self) -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .connect_timeout(REQUEST_TIMEOUT)
            .user_agent("Argus/0.1")
            .http1_only()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() > MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if self.allows(attempt.url()) {
                    attempt.follow()
                } else {
                    let host = attempt.url().host_str().unwrap_or("?").to_string();
                    attempt.error(format!("refusing a redirect to {host}, which is not a pinned mirror"))
                }
            }))
    }
}

/// An error with its causes: `reqwest` reports a refused redirect only as "error
/// following redirect", and the reason (ours) is in the source chain.
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

/// One mirror. `Ok(None)`: it has no such year (404, or an empty list).
async fn fetch_one(client: &reqwest::Client, url: &str, year: i32, pin: Pin) -> Result<Option<YearCalendar>, String> {
    let resp = client
        .get(url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {}", error_chain(&e)))?;
    // The client's policy already refuses an off-pin hop; this holds even if the client
    // handed in does not have it, since what was finally served is what gets believed.
    if !pin.allows(resp.url()) {
        return Err(format!("GET {url}: answered from {}, which is not a pinned mirror", resp.url().host_str().unwrap_or("?")));
    }
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("GET {url}: HTTP {status}"));
    }
    let body = crate::net::fetch_bytes_capped(resp, MAX_BODY_BYTES).await?;
    parse_year(&body, year).map_err(|e| format!("{url}: {e}"))
}

/// Walk the mirrors for one year. The first usable calendar wins; a mirror that
/// merely says "nothing yet" does not stop the next one being asked, since a
/// CDN can lag the origin by hours.
async fn fetch_year(client: &reqwest::Client, urls: &[String], year: i32, pin: Pin) -> Fetched {
    let mut unpublished = false;
    let mut last_error = String::from("no mirror configured");
    for url in urls {
        match fetch_one(client, url, year, pin).await {
            Ok(Some(cal)) => return Fetched::Calendar(cal),
            Ok(None) => unpublished = true,
            Err(e) => last_error = e,
        }
    }
    if unpublished {
        Fetched::Unpublished
    } else {
        Fetched::Failed(last_error)
    }
}

/// The years a refresh asks for: this one and the next, plus last year when the
/// cache holds no usable copy of it. Last year is what prices the usage recorded
/// before New Year (the reason [`KEEP_PAST_YEARS`] exists) and is where a year that
/// stopped passing validation ends up — [`read_cache`] drops it, and without this it
/// would never be asked for again once the calendar year had turned.
fn years_to_fetch(cache: &HolidayCache, now: i64) -> Vec<i32> {
    let year = beijing_year(now);
    let mut years = Vec::with_capacity(3);
    if !cache.years.iter().any(|y| y.year == year - 1) {
        years.push(year - 1);
    }
    years.push(year);
    years.push(year + 1);
    years
}

/// Ask the mirrors for the years in [`years_to_fetch`], and fold the answers into the
/// cache file. Returns whether the calendar changed.
async fn refresh_once(path: &Path, now: i64) -> bool {
    let Ok(client) = MIRROR_PIN.client_builder().build() else {
        return false;
    };
    let wanted = years_to_fetch(&read_cache(path), now);
    let mut fetched = Vec::new();
    let mut answered = false;
    for (i, y) in wanted.into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(BETWEEN_REQUESTS).await;
        }
        // Filtered here as well as pinned inside `fetch_one`, so a URL that is off the
        // pin is never even requested. The tests give `fetch_year` a local pin instead.
        let urls: Vec<String> = mirror_urls(y)
            .into_iter()
            .filter(|u| crate::net::validate_host_suffix(u, MIRROR_PIN.hosts).is_ok())
            .collect();
        match fetch_year(&client, &urls, y, MIRROR_PIN).await {
            Fetched::Calendar(cal) => {
                fetched.push(cal);
                answered = true;
            }
            Fetched::Unpublished => answered = true,
            Fetched::Failed(e) => eprintln!("[holidays] {y}: {e}"),
        }
    }
    if !answered {
        return false;
    }
    let mut cache = read_cache(path);
    let changed = merge(&mut cache, fetched, now, true);
    match write_cache(path, &cache) {
        Ok(()) => changed,
        Err(e) => {
            eprintln!("[holidays] could not write the cache: {e}");
            false
        }
    }
}

async fn check(app: &tauri::AppHandle) {
    let Ok(path) = cache_path(app) else {
        return;
    };
    let now = now_secs();
    if !needs_refresh(&read_cache(&path), now) {
        return;
    }
    if refresh_once(&path, now).await {
        let _ = app.emit(CHANGED_EVENT, serde_json::json!({}));
    }
}

/// Start the background refresh. Returns immediately.
pub fn spawn(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            check(&app).await;
            tokio::time::sleep(RECHECK_EVERY).await;
        }
    });
}

// ── Command ─────────────────────────────────────────────────────────────────

/// The cached calendar: no network, instant. Empty (`years: []`) before the
/// first successful refresh, in which case the frontend's embedded table and
/// statutory fallback answer.
#[tauri::command]
pub async fn get_cn_holidays(app: tauri::AppHandle) -> Result<HolidaySnapshot, String> {
    let path = cache_path(&app)?;
    let cache = tauri::async_runtime::spawn_blocking(move || read_cache(&path))
        .await
        .map_err(|e| format!("read holidays cache: {e}"))?;
    Ok(HolidaySnapshot { checked_at: cache.checked_at, years: cache.years })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 2026 file as holiday-cn serves it (fetched 2026-10-03).
    const FIXTURE_2026: &str = r#"{
    "$schema": "https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/schema.json",
    "$id": "https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/2026.json",
    "year": 2026,
    "papers": [
        "https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm"
    ],
    "days": [
        {"name": "元旦", "date": "2026-01-01", "isOffDay": true},
        {"name": "元旦", "date": "2026-01-02", "isOffDay": true},
        {"name": "元旦", "date": "2026-01-03", "isOffDay": true},
        {"name": "元旦", "date": "2026-01-04", "isOffDay": false},
        {"name": "春节", "date": "2026-02-14", "isOffDay": false},
        {"name": "春节", "date": "2026-02-15", "isOffDay": true},
        {"name": "春节", "date": "2026-02-16", "isOffDay": true},
        {"name": "春节", "date": "2026-02-17", "isOffDay": true},
        {"name": "春节", "date": "2026-02-18", "isOffDay": true},
        {"name": "春节", "date": "2026-02-19", "isOffDay": true},
        {"name": "春节", "date": "2026-02-20", "isOffDay": true},
        {"name": "春节", "date": "2026-02-21", "isOffDay": true},
        {"name": "春节", "date": "2026-02-22", "isOffDay": true},
        {"name": "春节", "date": "2026-02-23", "isOffDay": true},
        {"name": "春节", "date": "2026-02-28", "isOffDay": false},
        {"name": "清明节", "date": "2026-04-04", "isOffDay": true},
        {"name": "清明节", "date": "2026-04-05", "isOffDay": true},
        {"name": "清明节", "date": "2026-04-06", "isOffDay": true},
        {"name": "劳动节", "date": "2026-05-01", "isOffDay": true},
        {"name": "劳动节", "date": "2026-05-02", "isOffDay": true},
        {"name": "劳动节", "date": "2026-05-03", "isOffDay": true},
        {"name": "劳动节", "date": "2026-05-04", "isOffDay": true},
        {"name": "劳动节", "date": "2026-05-05", "isOffDay": true},
        {"name": "劳动节", "date": "2026-05-09", "isOffDay": false},
        {"name": "端午节", "date": "2026-06-19", "isOffDay": true},
        {"name": "端午节", "date": "2026-06-20", "isOffDay": true},
        {"name": "端午节", "date": "2026-06-21", "isOffDay": true},
        {"name": "国庆节", "date": "2026-09-20", "isOffDay": false},
        {"name": "中秋节", "date": "2026-09-25", "isOffDay": true},
        {"name": "中秋节", "date": "2026-09-26", "isOffDay": true},
        {"name": "中秋节", "date": "2026-09-27", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-01", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-02", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-03", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-04", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-05", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-06", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-07", "isOffDay": true},
        {"name": "国庆节", "date": "2026-10-10", "isOffDay": false}
    ]
}"#;

    /// 2027 as holiday-cn serves it before the State Council has published it.
    const FIXTURE_2027_EMPTY: &str = r#"{
    "$schema": "https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/schema.json",
    "$id": "https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/2027.json",
    "year": 2027,
    "papers": [],
    "days": []
}"#;

    fn day(date: &str, off: bool) -> HolidayDay {
        HolidayDay { date: date.into(), name: "节".into(), is_off_day: off }
    }

    /// `n` (at least 5) distinct days off in `year` that pass every rule: the five
    /// statutory days first, then Saturdays and Sundays from 1 February on, so the
    /// weekday count stays what the statutory days alone make it.
    fn days_off(year: i32, n: usize) -> Vec<HolidayDay> {
        use chrono::Datelike;
        assert!(n >= STATUTORY_OFF_DAYS.len());
        let mut out: Vec<HolidayDay> = STATUTORY_OFF_DAYS
            .iter()
            .map(|(m, d)| day(&format!("{year:04}-{m:02}-{d:02}"), true))
            .collect();
        let mut date = chrono::NaiveDate::from_ymd_opt(year, 2, 1).unwrap();
        while out.len() < n {
            if date.weekday().number_from_monday() > 5 {
                out.push(day(&date.format("%Y-%m-%d").to_string(), true));
            }
            date += chrono::Duration::days(1);
        }
        out
    }

    /// The 2025 arrangement as holiday-cn serves it (fetched 2026-10-03): 28 days off
    /// (18 of them weekdays) and 5 make-up days.
    const REAL_2025: [(&str, bool); 33] = [
        ("2025-01-01", true), ("2025-01-26", false), ("2025-01-28", true), ("2025-01-29", true), ("2025-01-30", true),
        ("2025-01-31", true), ("2025-02-01", true), ("2025-02-02", true), ("2025-02-03", true), ("2025-02-04", true),
        ("2025-02-08", false), ("2025-04-04", true), ("2025-04-05", true), ("2025-04-06", true), ("2025-04-27", false),
        ("2025-05-01", true), ("2025-05-02", true), ("2025-05-03", true), ("2025-05-04", true), ("2025-05-05", true),
        ("2025-05-31", true), ("2025-06-01", true), ("2025-06-02", true), ("2025-09-28", false), ("2025-10-01", true),
        ("2025-10-02", true), ("2025-10-03", true), ("2025-10-04", true), ("2025-10-05", true), ("2025-10-06", true),
        ("2025-10-07", true), ("2025-10-08", true), ("2025-10-11", false),
    ];

    fn real_2025() -> Vec<HolidayDay> {
        REAL_2025.iter().map(|(d, off)| day(d, *off)).collect()
    }

    fn real_2026() -> Vec<HolidayDay> {
        parse_year(FIXTURE_2026.as_bytes(), 2026).unwrap().unwrap().days
    }

    fn cal(year: i32, n: usize) -> YearCalendar {
        YearCalendar { year, days: days_off(year, n) }
    }

    fn body(year: i64, days: &[HolidayDay]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "year": year, "days": days })).unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("argus-holidays-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── parse / validate ──

    #[test]
    fn the_real_2026_file_parses_and_keeps_what_the_frontend_needs() {
        let cal = parse_year(FIXTURE_2026.as_bytes(), 2026).unwrap().expect("a calendar");
        assert_eq!(cal.year, 2026);
        assert_eq!(cal.days.len(), 39);
        assert_eq!(cal.days.iter().filter(|d| d.is_off_day).count(), 33);
        // National Day, including the Mon–Wed bridge days
        for d in ["2026-10-01", "2026-10-03", "2026-10-05", "2026-10-06", "2026-10-07"] {
            let hit = cal.days.iter().find(|x| x.date == d).unwrap_or_else(|| panic!("{d}"));
            assert!(hit.is_off_day, "{d}");
            assert_eq!(hit.name, "国庆节");
        }
        // make-up working days are kept, as not-off
        assert!(!cal.days.iter().find(|d| d.date == "2026-10-10").unwrap().is_off_day);
        // sorted by date
        assert!(cal.days.windows(2).all(|w| w[0].date < w[1].date));
    }

    #[test]
    fn an_unpublished_year_is_not_an_error_and_not_data() {
        assert_eq!(parse_year(FIXTURE_2027_EMPTY.as_bytes(), 2027), Ok(None));
    }

    #[test]
    fn a_file_for_another_year_is_rejected() {
        assert!(parse_year(FIXTURE_2026.as_bytes(), 2027).unwrap_err().contains("asked for 2027"));
        assert!(parse_year(FIXTURE_2027_EMPTY.as_bytes(), 2026).is_err());
    }

    #[test]
    fn garbage_is_rejected_not_panicked_on() {
        for junk in [
            &b""[..],
            b"<html><body>Please log in to the Wi-Fi</body></html>",
            b"null",
            b"[]",
            b"{}",
            b"{\"year\": 2026}",
            b"{\"year\": \"2026\", \"days\": []}",
            b"{\"year\": 2026, \"days\": \"none\"}",
            b"{\"year\": 2026, \"days\": [{}]}",
            b"{\"year\": 2026, \"days\": [{\"date\": \"2026-01-01\", \"name\": \"x\"}]}",
            b"{\"year\": 2026, \"days\": [{\"date\": \"2026-01-01\", \"name\": \"x\", \"isOffDay\": \"true\"}]}",
            b"{\"year\": 2026, \"days\": [{\"date\": 20260101, \"name\": \"x\", \"isOffDay\": true}]}",
            b"\xff\xfe\x00\x01",
        ] {
            assert!(parse_year(junk, 2026).is_err(), "{:?}", String::from_utf8_lossy(junk));
        }
    }

    #[test]
    fn dates_must_be_real_canonical_and_inside_the_year() {
        let mut days = days_off(2026, 20);
        for bad in [
            "2026-13-01", "2026-00-10", "2026-02-30", "2026-04-31", "2026-1-5", "2026-01-5", "2026/01/05",
            "20260105", "2026-01-05T00:00:00", " 2026-01-05", "2026-01-05 ", "26-01-05", "2025-12-31", "2027-01-01", "",
        ] {
            let mut d = days.clone();
            d.push(day(bad, true));
            assert!(parse_year(&body(2026, &d), 2026).is_err(), "accepted {bad:?}");
        }
        // a leap day is real only in a leap year
        days.push(day("2026-02-29", true));
        assert!(parse_year(&body(2026, &days), 2026).is_err());
        let mut leap = days_off(2028, 20);
        leap.push(day("2028-02-29", true));
        assert!(parse_year(&body(2028, &leap), 2028).is_ok());
    }

    #[test]
    fn duplicates_and_bad_names_are_rejected() {
        let mut d = days_off(2026, 20);
        d.push(day("2026-01-01", false)); // already there
        assert!(parse_year(&body(2026, &d), 2026).unwrap_err().contains("twice"));

        for name in ["", "   ", "a\u{0007}b", &"长".repeat(41)] {
            let mut d = days_off(2026, 20);
            d[3].name = name.to_string();
            assert!(parse_year(&body(2026, &d), 2026).is_err(), "accepted name {name:?}");
        }
        // padding is trimmed, a long-but-legal name is kept
        let mut d = days_off(2026, 20);
        d[0].name = "  国庆节、中秋节  ".into();
        let ok = parse_year(&body(2026, &d), 2026).unwrap().unwrap();
        assert_eq!(ok.days[0].name, "国庆节、中秋节");
    }

    #[test]
    fn the_number_of_days_off_must_be_plausible_for_a_year() {
        assert!(parse_year(&body(2026, &days_off(2026, MIN_OFF_DAYS - 1)), 2026).is_err());
        assert!(parse_year(&body(2026, &days_off(2026, MIN_OFF_DAYS)), 2026).is_ok());
        assert!(parse_year(&body(2026, &days_off(2026, MAX_OFF_DAYS)), 2026).is_ok());
        assert!(parse_year(&body(2026, &days_off(2026, MAX_OFF_DAYS + 1)), 2026).is_err());
        // make-up days do not count towards it
        let mut d = days_off(2026, 5);
        d.extend((6..16).map(|i| day(&format!("2026-02-{i:02}"), false)));
        assert!(parse_year(&body(2026, &d), 2026).is_err());
    }

    /// What the real calendars look like, as the rules see them. 2016–2024 were checked
    /// the same way by hand against holiday-cn's history: every year has the five
    /// statutory days off, 23–33 days off and 16–19 of them on a weekday.
    #[test]
    fn the_real_calendars_pass_the_rules_with_room_to_spare() {
        use chrono::Datelike;
        for (year, days) in [(2025, real_2025()), (2026, real_2026())] {
            assert!(validate_days(year, &days).unwrap().is_some(), "{year}");
            let weekday_off = days
                .iter()
                .filter(|d| d.is_off_day && chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").unwrap().weekday().number_from_monday() <= 5)
                .count();
            assert!(weekday_off + 5 <= MAX_WEEKDAY_OFF_DAYS, "{year}: {weekday_off} weekdays off leaves too little headroom");
        }
        // Labour Day is a day off in both, though on different weekdays.
        for days in [real_2025(), real_2026()] {
            assert!(days.iter().any(|d| d.date.ends_with("-05-01") && d.is_off_day));
        }
    }

    /// The reviewer's scenario: a response that is well-formed JSON for the right
    /// year, with a plausible *number* of days off, but not the arrangement. It used
    /// to pass and replace the verified embedded table for the whole year.
    #[test]
    fn a_calendar_missing_a_statutory_day_is_rejected() {
        for (month, day_of_month) in STATUTORY_OFF_DAYS {
            let date = format!("2026-{month:02}-{day_of_month:02}");
            // Marked as a working day…
            let mut days = real_2026();
            days.iter_mut().find(|d| d.date == date).unwrap().is_off_day = false;
            let err = parse_year(&body(2026, &days), 2026).unwrap_err();
            assert!(err.contains("statutory") && err.contains(&date), "{date} as a working day: {err}");
            // …or absent altogether.
            let mut days = real_2026();
            days.retain(|d| d.date != date);
            let err = parse_year(&body(2026, &days), 2026).unwrap_err();
            assert!(err.contains("statutory") && err.contains(&date), "{date} absent: {err}");
        }
        // The real files are untouched by the rule.
        assert!(parse_year(&body(2026, &real_2026()), 2026).unwrap().is_some());
    }

    #[test]
    fn weekdays_marked_off_are_capped() {
        use chrono::Datelike;
        // The five statutory days, then ordinary weekdays: 40 of them is the poisoned file.
        let weekdays_off = |year: i32, extra: usize| {
            let mut out = days_off(year, STATUTORY_OFF_DAYS.len());
            let mut date = chrono::NaiveDate::from_ymd_opt(year, 3, 2).unwrap(); // a Monday
            while out.len() < STATUTORY_OFF_DAYS.len() + extra {
                if date.weekday().number_from_monday() <= 5 {
                    out.push(day(&date.format("%Y-%m-%d").to_string(), true));
                }
                date += chrono::Duration::days(1);
            }
            out
        };
        // Five statutory weekdays in 2026 are 01-01 Thu, 05-01 Fri, 10-01 Thu, 10-02 Fri
        // (10-03 is a Saturday): four of them count.
        let at_cap = MAX_WEEKDAY_OFF_DAYS - 4;
        assert!(parse_year(&body(2026, &weekdays_off(2026, at_cap)), 2026).is_ok());
        let err = parse_year(&body(2026, &weekdays_off(2026, at_cap + 1)), 2026).unwrap_err();
        assert!(err.contains("weekdays off"), "{err}");
        let err = parse_year(&body(2026, &weekdays_off(2026, 40)), 2026).unwrap_err();
        assert!(err.contains("weekdays off") || err.contains("plausible"), "{err}");
        // Weekend days off cost nothing: a year with many of them is not capped.
        assert!(parse_year(&body(2026, &days_off(2026, MAX_OFF_DAYS)), 2026).is_ok());
    }

    #[test]
    fn a_huge_file_is_rejected_by_size_and_by_entry_count() {
        // valid JSON, a megabyte of padding
        let mut padded = FIXTURE_2026.as_bytes().to_vec();
        padded.extend(std::iter::repeat(b' ').take(MAX_BODY_BYTES as usize));
        assert!(parse_year(&padded, 2026).unwrap_err().contains("too large"));
        // small, but with far too many entries
        let many: Vec<HolidayDay> = (0..MAX_DAYS + 1)
            .map(|i| day(&(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i as i64)).format("%Y-%m-%d").to_string(), true))
            .collect();
        assert!(parse_year(&body(2026, &many), 2026).unwrap_err().contains("at most"));
    }

    // ── time ──

    #[test]
    fn the_year_turns_at_midnight_beijing_not_utc() {
        // 2026-12-31T15:59:59Z is 23:59:59 in Beijing; one second later it is 2027.
        let t = chrono::DateTime::parse_from_rfc3339("2026-12-31T15:59:59Z").unwrap().timestamp();
        assert_eq!(beijing_year(t), 2026);
        assert_eq!(beijing_year(t + 1), 2027);
        assert_eq!(beijing_year(0), 1970);
    }

    // ── freshness / merge ──

    fn cache_with(years: &[i32], checked_at: i64) -> HolidayCache {
        HolidayCache { version: CACHE_VERSION, checked_at, years: years.iter().map(|y| cal(*y, 20)).collect() }
    }

    const NOW: i64 = 1_790_000_000; // 2026-09-21 in Beijing

    #[test]
    fn an_empty_cache_needs_a_refresh() {
        assert!(needs_refresh(&HolidayCache::default(), NOW));
    }

    #[test]
    fn a_fresh_cache_with_the_current_year_is_left_alone() {
        assert_eq!(beijing_year(NOW), 2026);
        let c = cache_with(&[2026], NOW - 3600);
        assert!(!needs_refresh(&c, NOW));
        assert!(!needs_refresh(&cache_with(&[2026, 2027], NOW - REFRESH_AFTER_SECS + 1), NOW));
    }

    #[test]
    fn a_cache_goes_stale_after_a_few_days() {
        assert!(needs_refresh(&cache_with(&[2026], NOW - REFRESH_AFTER_SECS), NOW));
        assert!(needs_refresh(&cache_with(&[2026], NOW - 30 * 24 * 3600), NOW));
    }

    #[test]
    fn a_cache_without_the_current_year_is_refreshed_even_if_just_checked() {
        // e.g. the app ran across New Year, or only last year was ever cached
        assert!(needs_refresh(&cache_with(&[2025], NOW - 60), NOW));
        assert!(needs_refresh(&cache_with(&[2027], NOW - 60), NOW));
    }

    #[test]
    fn a_timestamp_from_the_future_does_not_silence_refreshes() {
        assert!(needs_refresh(&cache_with(&[2026], NOW + 365 * 24 * 3600), NOW));
        assert!(needs_refresh(&cache_with(&[2026], NOW + 1), NOW));
    }

    #[test]
    fn merge_replaces_a_year_and_reports_a_real_change() {
        let mut c = cache_with(&[2026], NOW - 5 * 24 * 3600);
        let changed = merge(&mut c, vec![cal(2026, 25), cal(2027, 22)], NOW, true);
        assert!(changed);
        assert_eq!(c.years.iter().map(|y| y.year).collect::<Vec<_>>(), vec![2026, 2027]);
        assert_eq!(c.years[0].days.len(), 25);
        assert_eq!(c.checked_at, NOW);
        assert_eq!(c.version, CACHE_VERSION);
    }

    #[test]
    fn merging_what_is_already_there_is_not_a_change_but_is_a_check() {
        let mut c = cache_with(&[2026, 2027], 100);
        let again = vec![cal(2026, 20), cal(2027, 20)];
        assert!(!merge(&mut c, again, NOW, true), "windows must not reload for nothing");
        assert_eq!(c.checked_at, NOW, "the check itself is still recorded");
    }

    #[test]
    fn a_failed_check_does_not_move_the_clock() {
        let mut c = cache_with(&[2026], 100);
        assert!(!merge(&mut c, vec![], NOW, false));
        assert_eq!(c.checked_at, 100);
    }

    /// A cache written by an older build (looser rules) can hold a year that no longer
    /// passes. `merge` must not carry it forward, and the refresh must ask for it again.
    #[test]
    fn merge_drops_a_cached_year_that_fails_the_rules_and_the_refresh_asks_for_it_again() {
        // Twenty consecutive days off from 1 January: plausible by the old count rule alone.
        let poisoned = YearCalendar {
            year: 2025,
            days: (0..20)
                .map(|i| day(&(chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Duration::days(i)).format("%Y-%m-%d").to_string(), true))
                .collect(),
        };
        assert!(validate_days(2025, &poisoned.days).is_err());
        let mut c = HolidayCache { version: CACHE_VERSION, checked_at: NOW - 60, years: vec![poisoned.clone(), cal(2026, 20)] };
        assert!(merge(&mut c, vec![], NOW, true), "dropping a year is a change windows must hear about");
        assert_eq!(c.years.iter().map(|y| y.year).collect::<Vec<_>>(), vec![2026]);

        // On disk the same year is dropped by `read_cache`, and the refresh wants it back.
        let dir = temp_dir("poisoned");
        let path = dir.join(CACHE_FILE);
        let on_disk = HolidayCache { version: CACHE_VERSION, checked_at: NOW - 60, years: vec![poisoned, cal(2026, 20)] };
        std::fs::write(&path, serde_json::to_string(&on_disk).unwrap()).unwrap();
        let back = read_cache(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.years.iter().map(|y| y.year).collect::<Vec<_>>(), vec![2026]);
        assert_eq!(years_to_fetch(&back, NOW), vec![2025, 2026, 2027]);
        // A good fetched copy then replaces it.
        let mut back = back;
        assert!(merge(&mut back, vec![cal(2025, 20)], NOW, true));
        assert_eq!(back.years.iter().map(|y| y.year).collect::<Vec<_>>(), vec![2025, 2026]);
    }

    #[test]
    fn last_year_is_fetched_only_while_the_cache_lacks_it() {
        assert_eq!(beijing_year(NOW), 2026);
        assert_eq!(years_to_fetch(&HolidayCache::default(), NOW), vec![2025, 2026, 2027]);
        assert_eq!(years_to_fetch(&cache_with(&[2025], NOW), NOW), vec![2026, 2027]);
        assert_eq!(years_to_fetch(&cache_with(&[2024, 2025, 2026, 2027], NOW), NOW), vec![2026, 2027]);
    }

    #[test]
    fn merge_keeps_other_years_and_forgets_the_very_old() {
        let mut c = cache_with(&[2022, 2023, 2024, 2025, 2026], 100);
        let changed = merge(&mut c, vec![cal(2027, 20)], NOW, true);
        assert!(changed);
        // current year is 2026; KEEP_PAST_YEARS = 2 keeps 2024 onwards
        assert_eq!(c.years.iter().map(|y| y.year).collect::<Vec<_>>(), vec![2024, 2025, 2026, 2027]);
    }

    // ── cache file ──

    #[test]
    fn the_cache_round_trips_through_the_file() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(CACHE_FILE);
        let mut c = HolidayCache::default();
        merge(&mut c, vec![parse_year(FIXTURE_2026.as_bytes(), 2026).unwrap().unwrap()], NOW, true);
        write_cache(&path, &c).unwrap();
        let back = read_cache(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back, c);
    }

    #[test]
    fn write_creates_the_directory() {
        let dir = temp_dir("mkdir");
        let path = dir.join("nested").join("deeper").join(CACHE_FILE);
        write_cache(&path, &HolidayCache::default()).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_cache_is_no_cache() {
        let dir = temp_dir("garbage");
        let path = dir.join(CACHE_FILE);
        assert_eq!(read_cache(&path), HolidayCache::default(), "missing file");
        for junk in ["", "not json", "[]", "{\"version\": 99, \"checkedAt\": 5, \"years\": []}", "{\"years\": 7}"] {
            std::fs::write(&path, junk).unwrap();
            assert_eq!(read_cache(&path), HolidayCache::default(), "{junk:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tampered_year_is_dropped_but_its_neighbours_survive() {
        let dir = temp_dir("tamper");
        let path = dir.join(CACHE_FILE);
        let mut c = cache_with(&[2025, 2026, 2027], NOW);
        c.years[1].days[2].date = "2031-05-05".into(); // belongs to another year
        c.years[2].days.truncate(2);                      // implausibly few days off
        std::fs::write(&path, serde_json::to_string(&c).unwrap()).unwrap();
        let back = read_cache(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.years.iter().map(|y| y.year).collect::<Vec<_>>(), vec![2025]);
        assert_eq!(back.checked_at, NOW);
    }

    #[test]
    fn a_duplicated_year_in_the_file_keeps_one() {
        let dir = temp_dir("dupyear");
        let path = dir.join(CACHE_FILE);
        let mut c = cache_with(&[2026], NOW);
        c.years.push(cal(2026, 30));
        std::fs::write(&path, serde_json::to_string(&c).unwrap()).unwrap();
        let back = read_cache(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.years.len(), 1);
    }

    #[test]
    fn the_snapshot_has_the_shape_the_frontend_reads() {
        let snap = HolidaySnapshot {
            checked_at: 7,
            years: vec![parse_year(FIXTURE_2026.as_bytes(), 2026).unwrap().unwrap()],
        };
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["checkedAt"], 7);
        assert_eq!(v["years"][0]["year"], 2026);
        assert_eq!(v["years"][0]["days"][0]["date"], "2026-01-01");
        assert_eq!(v["years"][0]["days"][0]["name"], "元旦");
        assert_eq!(v["years"][0]["days"][0]["isOffDay"], true);
        assert!(v["years"][0]["days"][0].get("is_off_day").is_none());
    }

    // ── mirrors ──

    #[test]
    fn the_mirrors_are_https_pinned_to_their_hosts_and_in_order() {
        let urls = mirror_urls(2027);
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("https://cdn.jsdelivr.net/"), "jsDelivr is tried first");
        assert!(urls[1].starts_with("https://raw.githubusercontent.com/"));
        for u in &urls {
            assert!(u.ends_with("/2027.json"), "{u}");
            assert!(crate::net::validate_host_suffix(u, &MIRROR_HOSTS).is_ok(), "{u}");
        }
        assert!(crate::net::validate_host_suffix("https://evil.example/2027.json", &MIRROR_HOSTS).is_err());
    }

    // ── fetch_year against a local server ──

    /// Serve each canned response once, in order, one per connection.
    async fn serve(responses: Vec<(u16, Vec<u8>)>) -> (String, tokio::task::JoinHandle<()>) {
        serve_with_locations(responses.into_iter().map(|(status, body)| (status, None, body)).collect()).await
    }

    /// As [`serve`], with an optional `Location` header (for redirects).
    async fn serve_with_locations(responses: Vec<(u16, Option<String>, Vec<u8>)>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            for (status, location, payload) in responses {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let location = location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default();
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&payload).await;
                let _ = sock.shutdown().await;
            }
        });
        (base, handle)
    }

    /// The pin the local-server tests run under: loopback, plain http.
    const LOCAL: Pin = Pin { hosts: &["127.0.0.1"], https_only: false };

    /// Production's client policy, aimed at loopback (and without a system proxy, which
    /// would swallow localhost).
    fn plain_client() -> reqwest::Client {
        LOCAL.client_builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn the_first_good_mirror_wins() {
        let (base, h) = serve(vec![(200, FIXTURE_2026.as_bytes().to_vec())]).await;
        let urls = vec![format!("{base}/2026.json"), format!("{base}/never-asked.json")];
        match fetch_year(&plain_client(), &urls, 2026, LOCAL).await {
            Fetched::Calendar(c) => assert_eq!(c.days.len(), 39),
            _ => panic!("expected a calendar"),
        }
        h.abort();
    }

    #[tokio::test]
    async fn a_failing_or_garbage_mirror_falls_through_to_the_next() {
        let (base, h) = serve(vec![
            (500, b"oops".to_vec()),
            (200, b"<html>captive portal</html>".to_vec()),
            (200, FIXTURE_2026.as_bytes().to_vec()),
        ])
        .await;
        let urls: Vec<String> = (0..3).map(|i| format!("{base}/m{i}/2026.json")).collect();
        assert!(matches!(fetch_year(&plain_client(), &urls, 2026, LOCAL).await, Fetched::Calendar(_)));
        h.abort();
    }

    #[tokio::test]
    async fn a_wrong_year_from_a_mirror_is_not_accepted() {
        let (base, h) = serve(vec![(200, FIXTURE_2026.as_bytes().to_vec())]).await;
        let urls = vec![format!("{base}/2027.json")];
        // asked for 2027, the server answers with the 2026 file
        assert!(matches!(fetch_year(&plain_client(), &urls, 2027, LOCAL).await, Fetched::Failed(_)));
        h.abort();
    }

    #[tokio::test]
    async fn not_found_and_empty_mean_unpublished_not_failure() {
        let (base, h) = serve(vec![(404, b"Not Found".to_vec()), (200, FIXTURE_2027_EMPTY.as_bytes().to_vec())]).await;
        let urls = vec![format!("{base}/a/2027.json"), format!("{base}/b/2027.json")];
        assert!(matches!(fetch_year(&plain_client(), &urls, 2027, LOCAL).await, Fetched::Unpublished));
        h.abort();
    }

    #[tokio::test]
    async fn a_mirror_that_lags_does_not_hide_one_that_has_the_year() {
        let (base, h) = serve(vec![(200, FIXTURE_2027_EMPTY.as_bytes().to_vec()), (200, {
            let days: Vec<HolidayDay> = days_off(2027, 20);
            body(2027, &days)
        })])
        .await;
        let urls = vec![format!("{base}/a/2027.json"), format!("{base}/b/2027.json")];
        assert!(matches!(fetch_year(&plain_client(), &urls, 2027, LOCAL).await, Fetched::Calendar(_)));
        h.abort();
    }

    #[tokio::test]
    async fn an_oversized_response_is_refused() {
        let mut big = FIXTURE_2026.as_bytes().to_vec();
        big.extend(std::iter::repeat(b' ').take(MAX_BODY_BYTES as usize + 10));
        let (base, h) = serve(vec![(200, big)]).await;
        let urls = vec![format!("{base}/2026.json")];
        assert!(matches!(fetch_year(&plain_client(), &urls, 2026, LOCAL).await, Fetched::Failed(_)));
        h.abort();
    }

    #[tokio::test]
    async fn an_unreachable_mirror_is_a_quiet_failure() {
        // port 1 on localhost: connection refused
        let urls = vec!["http://127.0.0.1:1/2026.json".to_string()];
        assert!(matches!(fetch_year(&plain_client(), &urls, 2026, LOCAL).await, Fetched::Failed(_)));
    }

    // ── redirects ──

    #[test]
    fn the_pin_allows_only_https_on_the_mirror_hosts() {
        let allows = |u: &str| MIRROR_PIN.allows(&reqwest::Url::parse(u).unwrap());
        assert!(allows("https://cdn.jsdelivr.net/gh/NateScarlet/holiday-cn@master/2026.json"));
        assert!(allows("https://raw.githubusercontent.com/NateScarlet/holiday-cn/master/2026.json"));
        assert!(!allows("http://cdn.jsdelivr.net/2026.json"), "a downgrade to plain http");
        assert!(!allows("https://evil.example/2026.json"));
        assert!(!allows("https://cdn.jsdelivr.net.evil.example/2026.json"));
        assert!(!allows("https://evilcdn.jsdelivr.net.example/2026.json"));
        assert!(!allows("https://cdn.jsdelivr.net@evil.example/2026.json"), "userinfo trick");
        assert!(!allows("https://127.0.0.1/2026.json"));
        // And the production client is built from the same pin (not the shared, redirect-anywhere one).
        assert!(MIRROR_PIN.client_builder().build().is_ok());
    }

    #[tokio::test]
    async fn a_redirect_to_another_host_is_not_followed() {
        // The target would answer with a perfectly valid calendar, so only the refusal can fail this.
        let (target, th) = serve(vec![(200, FIXTURE_2026.as_bytes().to_vec())]).await;
        let off_pin = target.replace("127.0.0.1", "localhost");
        let (base, h) = serve_with_locations(vec![(302, Some(format!("{off_pin}/2026.json")), vec![])]).await;
        let urls = vec![format!("{base}/2026.json")];
        match fetch_year(&plain_client(), &urls, 2026, LOCAL).await {
            Fetched::Failed(e) => assert!(e.contains("not a pinned mirror"), "{e}"),
            _ => panic!("a redirect off the pinned host was followed"),
        }
        // The off-pin server never saw a connection: it is still waiting to accept one.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!th.is_finished(), "the redirect target was contacted");
        th.abort();
        h.abort();
    }

    #[tokio::test]
    async fn a_redirect_that_stays_on_the_pinned_host_is_followed() {
        let (target, th) = serve(vec![(200, FIXTURE_2026.as_bytes().to_vec())]).await;
        let (base, h) = serve_with_locations(vec![(301, Some(format!("{target}/moved/2026.json")), vec![])]).await;
        let urls = vec![format!("{base}/2026.json")];
        assert!(matches!(fetch_year(&plain_client(), &urls, 2026, LOCAL).await, Fetched::Calendar(_)));
        th.abort();
        h.abort();
    }

    #[tokio::test]
    async fn a_redirect_loop_gives_up() {
        let (base, h) = serve_with_locations(
            (0..MAX_REDIRECTS + 4).map(|_| (302, Some("/loop.json".to_string()), vec![])).collect(),
        )
        .await;
        let urls = vec![format!("{base}/2026.json")];
        match fetch_year(&plain_client(), &urls, 2026, LOCAL).await {
            Fetched::Failed(e) => assert!(e.contains("redirect"), "{e}"),
            _ => panic!("a redirect loop was not stopped"),
        }
        h.abort();
    }

    /// Belt and braces: a client that *does* follow anywhere (the shared one's default)
    /// still cannot get an off-pin answer believed, since the final URL is checked.
    #[tokio::test]
    async fn an_answer_from_an_unpinned_host_is_refused_even_when_the_client_followed() {
        let (target, th) = serve(vec![(200, FIXTURE_2026.as_bytes().to_vec())]).await;
        let off_pin = target.replace("127.0.0.1", "localhost");
        let (base, h) = serve_with_locations(vec![(302, Some(format!("{off_pin}/2026.json")), vec![])]).await;
        let following_anywhere = reqwest::Client::builder().no_proxy().build().unwrap();
        let urls = vec![format!("{base}/2026.json")];
        match fetch_year(&following_anywhere, &urls, 2026, LOCAL).await {
            Fetched::Failed(e) => assert!(e.contains("not a pinned mirror"), "{e}"),
            Fetched::Calendar(_) => panic!("a calendar from an unpinned host was accepted"),
            Fetched::Unpublished => panic!("unexpected: unpublished"),
        }
        th.abort();
        h.abort();
    }

    /// Against the real mirrors; run by hand: `cargo test --lib holidays -- --ignored live`.
    #[tokio::test]
    #[ignore]
    async fn live_the_real_mirrors_serve_valid_calendars() {
        let client = MIRROR_PIN.client_builder().build().unwrap();
        for year in [2025, 2026] {
            for url in mirror_urls(year) {
                let cal = fetch_one(&client, &url, year, MIRROR_PIN).await.unwrap_or_else(|e| panic!("{e}")).expect("published");
                assert!(cal.days.iter().filter(|d| d.is_off_day).count() >= 28, "{url}");
            }
        }
        // 2027 is either unpublished (None) or a valid calendar, never an error
        for url in mirror_urls(2027) {
            fetch_one(&client, &url, 2027, MIRROR_PIN).await.unwrap_or_else(|e| panic!("{e}"));
        }
    }
}
