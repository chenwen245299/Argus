//! Bounded, read-only queries over the current library's usage log. No provider
//! configuration is loaded: costs are only those actually stored in the log.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::UsageRecord;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    None,
    Hour,
    #[default]
    Day,
    Month,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UsageQuery {
    /// Inclusive start, RFC3339 with an explicit timezone, e.g. 2026-10-20T00:00:00+08:00.
    /// Omit for all recorded history. Filtering uses the recorded timestamp.
    pub start_time: Option<String>,
    /// Exclusive end, RFC3339 with an explicit timezone. To query a whole day,
    /// use midnight of the following day. Omit for no upper bound.
    pub end_time: Option<String>,
    /// Exact provider identifier as stored in usage records, NOT a display name.
    /// Omit initially to discover identifiers in by_provider.
    pub provider: Option<String>,
    /// Exact model identifier. Omit initially to discover models in by_model.
    pub model: Option<String>,
    /// Exact recorded task source, e.g. library-agent, copilot, user-memory.
    /// Omit initially to discover sources in by_source.
    pub source: Option<String>,
    /// Timeline bucket size: none, hour, day (default), or month. Empty buckets
    /// are omitted. Summaries and breakdowns always cover ALL matching records.
    pub granularity: Granularity,
    /// Fixed UTC offset in minutes for timeline buckets only; default 0 (UTC).
    /// Use 480 for Singapore/China. Range -840..840. No daylight-saving rules.
    pub timezone_offset_minutes: i32,
    /// Include individual usage records (default true). Set false for summaries
    /// only, without changing totals, breakdowns or timeline.
    pub include_records: bool,
    /// Record order by actual timestamp: desc (newest first, default) or asc.
    /// Equal timestamps retain log order. Does not change timeline order.
    pub order: Order,
    /// Number of matching records to skip. Default 0. Pagination is a fresh
    /// snapshot per call; use a fixed end_time and asc order for append-only logs.
    pub offset: usize,
    /// Max individual records returned. Default 100; clamped to 1..1000.
    pub limit: usize,
}

impl Default for UsageQuery {
    fn default() -> Self {
        Self {
            start_time: None,
            end_time: None,
            provider: None,
            model: None,
            source: None,
            granularity: Granularity::Day,
            timezone_offset_minutes: 0,
            include_records: true,
            order: Order::Desc,
            offset: 0,
            limit: 100,
        }
    }
}

#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
pub struct Totals {
    /// Number of logged usage records, not user messages (one answer may make
    /// multiple model calls). Calls reporting no tokens/cost may not be logged.
    pub records: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// input_tokens + output_tokens. Cache hits are already part of input.
    pub total_tokens: u64,
    pub cache_hit_tokens: u64,
    /// input_tokens - cache_hit_tokens.
    pub uncached_input_tokens: u64,
    /// cache_hit_tokens / input_tokens; null when there are no input tokens.
    /// Legacy records without a cache count contribute zero known cache hits.
    pub cache_hit_rate: Option<f64>,
    /// Sum of recorded USD costs only; null if no matching record has a cost.
    /// Missing costs are NOT estimated and this is NOT a full bill when
    /// records_without_cost > 0. An explicitly recorded zero is a known cost.
    pub reported_cost_usd: Option<f64>,
    pub records_with_cost: u64,
    pub records_without_cost: u64,
    pub first_record_at: Option<String>,
    pub last_record_at: Option<String>,
}

impl Totals {
    fn add(&mut self, record: &UsageEntry) {
        self.records += 1;
        self.input_tokens = self.input_tokens.saturating_add(record.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(record.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(record.total_tokens);
        self.cache_hit_tokens = self
            .cache_hit_tokens
            .saturating_add(record.cache_hit_tokens);
        self.uncached_input_tokens = self
            .uncached_input_tokens
            .saturating_add(record.uncached_input_tokens);
        self.cache_hit_rate = (self.input_tokens > 0)
            .then(|| self.cache_hit_tokens as f64 / self.input_tokens as f64);
        if let Some(cost) = record.cost_usd {
            *self.reported_cost_usd.get_or_insert(0.0) += cost;
            self.records_with_cost += 1;
        } else {
            self.records_without_cost += 1;
        }
    }

    fn add_at(&mut self, record: &UsageEntry, ts: DateTime<Utc>, bounds: &mut Bounds) {
        self.add(record);
        bounds.add(ts);
    }

    fn finish(&mut self, bounds: &Bounds) {
        self.first_record_at = bounds.first.map(timestamp);
        self.last_record_at = bounds.last.map(timestamp);
    }
}

#[derive(Default)]
struct Bounds {
    first: Option<DateTime<Utc>>,
    last: Option<DateTime<Utc>>,
}

impl Bounds {
    fn add(&mut self, ts: DateTime<Utc>) {
        self.first = Some(self.first.map_or(ts, |old| old.min(ts)));
        self.last = Some(self.last.map_or(ts, |old| old.max(ts)));
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UsageEntry {
    /// UTC RFC3339 timestamp from the usage log. It is the accounting time,
    /// not a request start time or duration.
    pub ts: String,
    pub source: String,
    /// Logged provider identifier only; no credentials, endpoints or configuration.
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cache_hit_tokens: u64,
    pub uncached_input_tokens: u64,
    /// Recorded USD cost, or null when unavailable. No price-based estimates.
    pub cost_usd: Option<f64>,
}

impl UsageEntry {
    fn new(record: UsageRecord, ts: DateTime<Utc>) -> Self {
        let cache_hit_tokens = record.cache_hit_tokens.min(record.input_tokens);
        Self {
            ts: timestamp(ts),
            source: record.source,
            provider: record.provider,
            model: record.model,
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            total_tokens: record.input_tokens.saturating_add(record.output_tokens),
            cache_hit_tokens,
            uncached_input_tokens: record.input_tokens - cache_hit_tokens,
            cost_usd: record.cost_usd.filter(|v| v.is_finite() && *v >= 0.0),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Breakdown {
    /// Recorded provider, model or source identifier, depending on the array.
    pub key: String,
    pub totals: Totals,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct TimeBucket {
    /// Local bucket label in the requested fixed offset: YYYY-MM for month,
    /// YYYY-MM-DD for day, or RFC3339 at the start of the hour for hour.
    pub bucket: String,
    pub totals: Totals,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct UsageReport {
    /// Effective query, with UTC-normalized bounds and the clamped page limit.
    /// The scope is always this library's log on disk, not account-wide usage.
    pub query: UsageQuery,
    /// Entire matching range, independent of the records page.
    pub summary: Totals,
    pub by_provider: Vec<Breakdown>,
    pub by_model: Vec<Breakdown>,
    pub by_source: Vec<Breakdown>,
    /// Oldest bucket first. Empty when granularity is none.
    pub timeline: Vec<TimeBucket>,
    pub records: Vec<UsageEntry>,
    pub returned: usize,
    /// More matching detail records exist after this page. False if details disabled.
    pub has_more: bool,
    /// Next detail offset, or null if there is no further page / details disabled.
    pub next_offset: Option<usize>,
    /// Invalid nonempty log lines (including an incomplete final append) skipped
    /// across the snapshot. They cannot be attributed to a particular filter.
    pub skipped_records: u64,
}

type Aggregate = (Totals, Bounds);
type Aggregates = BTreeMap<String, Aggregate>;

fn timestamp(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn parse_bound(value: Option<&str>, name: &str) -> Result<Option<DateTime<Utc>>, String> {
    value
        .map(|raw| {
            DateTime::parse_from_rfc3339(raw)
                .map(|ts| ts.with_timezone(&Utc))
                .map_err(|_| format!("{name} must be an RFC3339 timestamp with a timezone"))
        })
        .transpose()
}

fn add_group(groups: &mut Aggregates, key: &str, record: &UsageEntry, ts: DateTime<Utc>) {
    let (totals, bounds) = groups.entry(key.to_string()).or_default();
    totals.add_at(record, ts, bounds);
}

fn breakdown(groups: Aggregates) -> Vec<Breakdown> {
    groups
        .into_iter()
        .map(|(key, (mut totals, bounds))| {
            totals.finish(&bounds);
            Breakdown { key, totals }
        })
        .collect()
}

/// Read a single length-bounded snapshot so concurrent appends cannot keep a
/// query running forever. Missing logs are empty; other I/O errors are surfaced.
pub fn read(root: &str, mut query: UsageQuery) -> Result<UsageReport, String> {
    let start = parse_bound(query.start_time.as_deref(), "start_time")?;
    let end = parse_bound(query.end_time.as_deref(), "end_time")?;
    if matches!((start, end), (Some(a), Some(b)) if a >= b) {
        return Err("start_time must be earlier than end_time (end_time is exclusive)".into());
    }
    if !(-840..=840).contains(&query.timezone_offset_minutes) {
        return Err("timezone_offset_minutes must be between -840 and 840".into());
    }
    let timezone = FixedOffset::east_opt(query.timezone_offset_minutes * 60).unwrap();
    query.start_time = start.map(timestamp);
    query.end_time = end.map(timestamp);
    query.limit = query.limit.clamp(1, 1000);

    let mut summary = Totals::default();
    let mut bounds = Bounds::default();
    let mut providers = Aggregates::new();
    let mut models = Aggregates::new();
    let mut sources = Aggregates::new();
    let mut timeline = Aggregates::new();
    let mut entries = Vec::new();
    let mut skipped_records = 0;
    let file = match File::open(super::usage_path(root)) {
        Ok(file) => Some(file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot open usage log: {e}")),
    };
    if let Some(file) = file {
        let size = file
            .metadata()
            .map_err(|e| format!("cannot inspect usage log: {e}"))?
            .len();
        let mut reader = BufReader::new(file.take(size));
        let mut line = Vec::new();
        loop {
            line.clear();
            let len = reader
                .read_until(b'\n', &mut line)
                .map_err(|e| format!("cannot read usage log: {e}"))?;
            if len == 0 {
                break;
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let record = serde_json::from_slice::<UsageRecord>(&line).ok();
            let Some((record, ts)) = record.and_then(|record| {
                DateTime::parse_from_rfc3339(&record.ts)
                    .ok()
                    .map(|ts| (record, ts.with_timezone(&Utc)))
            }) else {
                skipped_records += 1;
                continue;
            };
            if start.is_some_and(|start| ts < start)
                || end.is_some_and(|end| ts >= end)
                || query
                    .provider
                    .as_ref()
                    .is_some_and(|v| v != &record.provider)
                || query.model.as_ref().is_some_and(|v| v != &record.model)
                || query.source.as_ref().is_some_and(|v| v != &record.source)
            {
                continue;
            }
            let entry = UsageEntry::new(record, ts);
            summary.add_at(&entry, ts, &mut bounds);
            add_group(&mut providers, &entry.provider, &entry, ts);
            add_group(&mut models, &entry.model, &entry, ts);
            add_group(&mut sources, &entry.source, &entry, ts);
            let local = ts.with_timezone(&timezone);
            let bucket = match query.granularity {
                Granularity::None => None,
                Granularity::Hour => Some(local.format("%Y-%m-%dT%H:00:00%:z").to_string()),
                Granularity::Day => Some(local.format("%Y-%m-%d").to_string()),
                Granularity::Month => Some(local.format("%Y-%m").to_string()),
            };
            if let Some(bucket) = bucket {
                add_group(&mut timeline, &bucket, &entry, ts);
            }
            if query.include_records {
                entries.push((ts, entry));
            }
        }
    }
    summary.finish(&bounds);
    // Stable sort keeps append order when several calls share one timestamp.
    entries.sort_by(|a, b| match query.order {
        Order::Asc => a.0.cmp(&b.0),
        Order::Desc => b.0.cmp(&a.0),
    });
    let available = entries.len().saturating_sub(query.offset);
    let records: Vec<_> = entries
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .map(|(_, entry)| entry)
        .collect();
    let returned = records.len();
    let has_more = available > returned;
    let next_offset = has_more.then(|| query.offset + returned);
    Ok(UsageReport {
        query,
        summary,
        by_provider: breakdown(providers),
        by_model: breakdown(models),
        by_source: breakdown(sources),
        timeline: breakdown(timeline)
            .into_iter()
            .map(|row| TimeBucket {
                bucket: row.key,
                totals: row.totals,
            })
            .collect(),
        records,
        returned,
        has_more,
        next_offset,
        skipped_records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::PathBuf;

    struct Log(PathBuf);
    impl Log {
        fn new(rows: Vec<Value>) -> Self {
            let dir =
                std::env::temp_dir().join(format!("argus-usage-query-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join(".argus")).unwrap();
            let log = Self(dir);
            let text = rows
                .into_iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>();
            std::fs::write(super::super::usage_path(&log.root()), text).unwrap();
            log
        }

        fn root(&self) -> String {
            self.0.to_string_lossy().to_string()
        }
        fn query(&self, args: Value) -> UsageReport {
            read(&self.root(), serde_json::from_value(args).unwrap()).unwrap()
        }
    }
    impl Drop for Log {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn row(ts: &str, provider: &str, model: &str, source: &str, input: u64, output: u64) -> Value {
        json!({"ts":ts, "provider":provider, "model":model, "source":source,
            "input_tokens":input, "output_tokens":output})
    }

    fn fixture() -> Log {
        let mut cached = row("2026-10-19T16:00:00Z", "p1", "m1", "paper-chat", 100, 20);
        cached["cache_hit_tokens"] = json!(60);
        cached["cost_usd"] = json!(0.25);
        let mut free = row("2026-10-20T15:59:59Z", "p2", "m2", "user-memory", 50, 10);
        free["cost_usd"] = json!(0);
        Log::new(vec![
            row("2026-10-19T15:59:59Z", "p1", "m1", "paper-chat", 7, 1),
            cached,
            row("2026-10-20T08:00:00Z", "p1", "m1", "library-chat", 200, 30),
            free,
            row("2026-10-20T16:00:00Z", "p1", "m1", "paper-chat", 9, 1),
        ])
    }

    fn day() -> Value {
        json!({"start_time":"2026-10-20T00:00:00+08:00",
            "end_time":"2026-10-21T00:00:00+08:00", "timezone_offset_minutes":480})
    }

    #[test]
    fn local_day_bounds_and_full_range_totals_survive_detail_paging() {
        let log = fixture();
        let mut args = day();
        args["limit"] = json!(1);
        let report = log.query(args.clone());
        assert_eq!(report.summary.records, 3);
        assert_eq!(report.summary.input_tokens, 350);
        assert_eq!(report.summary.output_tokens, 60);
        assert_eq!(report.summary.total_tokens, 410);
        assert_eq!(report.summary.cache_hit_tokens, 60);
        assert_eq!(report.summary.uncached_input_tokens, 290);
        assert_eq!(report.summary.cache_hit_rate, Some(60.0 / 350.0));
        assert_eq!(report.summary.reported_cost_usd, Some(0.25));
        assert_eq!(report.summary.records_with_cost, 2);
        assert_eq!(report.summary.records_without_cost, 1);
        assert_eq!(
            report.summary.first_record_at.as_deref(),
            Some("2026-10-19T16:00:00Z")
        );
        assert_eq!(
            report.summary.last_record_at.as_deref(),
            Some("2026-10-20T15:59:59Z")
        );
        assert_eq!(report.timeline.len(), 1);
        assert_eq!(report.timeline[0].bucket, "2026-10-20");
        assert_eq!(report.timeline[0].totals.records, 3);
        assert_eq!(report.by_provider.len(), 2);
        assert_eq!(report.by_model.len(), 2);
        assert_eq!(report.by_source.len(), 3);
        assert_eq!(report.records[0].model, "m2");
        assert_eq!(report.returned, 1);
        assert!(report.has_more);
        assert_eq!(report.next_offset, Some(1));
        assert_eq!(
            report.query.start_time.as_deref(),
            Some("2026-10-19T16:00:00Z")
        );
        args["offset"] = json!(2);
        let last = log.query(args.clone());
        assert_eq!(last.summary.total_tokens, 410);
        assert_eq!(last.records[0].cache_hit_tokens, 60);
        assert!(!last.has_more);
        assert_eq!(last.next_offset, None);
        args["offset"] = json!(usize::MAX);
        let past = log.query(args);
        assert_eq!(past.returned, 0);
        assert!(!past.has_more);
    }

    #[test]
    fn exact_filters_combine_and_missing_costs_are_not_free() {
        let log = fixture();
        let report = log.query(json!({"provider":"p1", "model":"m1", "source":"library-chat"}));
        assert_eq!(report.summary.records, 1);
        assert_eq!(report.summary.reported_cost_usd, None);
        assert_eq!(report.summary.records_without_cost, 1);
        assert_eq!(report.records[0].cache_hit_tokens, 0); // legacy field absent
        assert_eq!(report.records[0].uncached_input_tokens, 200);
        let free = log.query(json!({"provider":"p2"}));
        assert_eq!(free.summary.reported_cost_usd, Some(0.0));
        assert_eq!(free.summary.records_with_cost, 1);
        let no_match = log.query(json!({"provider":"P1"}));
        assert_eq!(no_match.summary.records, 0);
        assert_eq!(no_match.summary.cache_hit_rate, None);
        assert!(no_match.by_provider.is_empty());
    }

    #[test]
    fn chronological_sort_uses_instants_and_equal_times_keep_log_order() {
        let log = Log::new(vec![
            row("2026-10-20T09:00:00+08:00", "p", "later", "s", 1, 1),
            row("2026-10-20T00:59:59.900Z", "p", "earlier", "s", 1, 1),
            row("2026-10-20T01:00:00Z", "p", "same", "s", 1, 1),
        ]);
        let desc = log.query(json!({}));
        assert_eq!(
            desc.records
                .iter()
                .map(|r| r.model.as_str())
                .collect::<Vec<_>>(),
            ["later", "same", "earlier"]
        );
        let asc = log.query(json!({"order":"asc", "limit":2}));
        assert_eq!(
            asc.records
                .iter()
                .map(|r| r.model.as_str())
                .collect::<Vec<_>>(),
            ["earlier", "later"]
        );
        assert_eq!(asc.next_offset, Some(2));
    }

    #[test]
    fn summaries_only_and_all_timeline_granularities() {
        let log = fixture();
        let report = log.query(json!({"include_records":false, "granularity":"month"}));
        assert_eq!(report.summary.records, 5);
        assert_eq!(report.timeline[0].bucket, "2026-10");
        assert_eq!(report.timeline[0].totals.records, 5);
        assert_eq!(report.returned, 0);
        assert!(!report.has_more);
        assert_eq!(report.next_offset, None);
        let none = log.query(json!({"granularity":"none"}));
        assert!(none.timeline.is_empty());
        let hours = log.query(json!({"granularity":"hour", "timezone_offset_minutes":480}));
        assert_eq!(hours.timeline[0].bucket, "2026-10-19T23:00:00+08:00");
        assert_eq!(hours.timeline[1].bucket, "2026-10-20T00:00:00+08:00");
        let west = log.query(json!({"granularity":"day", "timezone_offset_minutes":-480}));
        assert_eq!(west.timeline[0].bucket, "2026-10-19");
        assert_eq!(west.timeline[0].totals.records, 2);
        assert_eq!(log.query(json!({"limit":0})).query.limit, 1);
        assert_eq!(log.query(json!({"limit":10001})).query.limit, 1000);
    }

    #[test]
    fn malformed_lines_and_partial_appends_are_visible_without_blocking_valid_history() {
        let mut bad_cache = row("2026-10-20T00:00:00Z", "p", "m", "s", 10, 2);
        bad_cache["cache_hit_tokens"] = json!(900);
        bad_cache["cost_usd"] = json!(-1);
        let log = Log::new(vec![bad_cache, row("bad timestamp", "p", "m", "s", 4, 2)]);
        use std::io::Write;
        let path = super::super::usage_path(&log.root());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\nnot json\n{\"ts\":").unwrap();
        let before = std::fs::read(&path).unwrap();
        let report = log.query(json!({}));
        assert_eq!(report.summary.records, 1);
        assert_eq!(report.skipped_records, 3);
        assert_eq!(report.summary.cache_hit_tokens, 10);
        assert_eq!(report.summary.uncached_input_tokens, 0);
        assert_eq!(report.summary.reported_cost_usd, None);
        assert_eq!(std::fs::read(&path).unwrap(), before); // read-only, no repair
    }

    #[test]
    fn invalid_ranges_timezones_and_unknown_arguments_fail_explicitly() {
        let log = fixture();
        for args in [
            json!({"start_time":"2026-10-20"}),
            json!({"start_time":"2026-10-20T00:00:00"}),
            json!({"end_time":"bad"}),
            json!({"start_time":"2026-10-21T00:00:00Z", "end_time":"2026-10-20T00:00:00Z"}),
            json!({"start_time":"2026-10-20T00:00:00Z", "end_time":"2026-10-20T08:00:00+08:00"}),
            json!({"timezone_offset_minutes":841}),
            json!({"timezone_offset_minutes":-841}),
        ] {
            assert!(
                read(&log.root(), serde_json::from_value(args.clone()).unwrap()).is_err(),
                "{args}"
            );
        }
        assert!(serde_json::from_value::<UsageQuery>(json!({"start":"typo"})).is_err());
        assert!(serde_json::from_value::<UsageQuery>(json!({"granularity":"week"})).is_err());
    }

    #[test]
    fn missing_log_is_empty_but_io_errors_are_not_silent_and_libraries_are_isolated() {
        let log = fixture();
        let other = Log::new(vec![row("2026-10-20T00:00:00Z", "other", "m", "s", 999, 0)]);
        let result = crate::mcp::agent::call(&other.root(), "get_token_usage", &json!({})).unwrap();
        assert_eq!(result["summary"]["input_tokens"], 999);
        assert_eq!(log.query(json!({})).summary.input_tokens, 366);
        let missing = other.0.join("missing");
        let report = read(&missing.to_string_lossy(), UsageQuery::default()).unwrap();
        assert_eq!(report.summary.records, 0);
        assert!(!missing.exists());
        let path = super::super::usage_path(&other.root());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(read(&other.root(), UsageQuery::default()).is_err());
    }
}
