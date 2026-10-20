# AI Workflows

Argus is **AI-native**: AI isn't a bolt-on feature or a plugin — it runs throughout the
app. Just configure your preferred AI providers and models to use it.

## Supported AI provider API formats

Under **Settings → AI Providers** you can configure one or more providers at once. Argus
supports these request formats:

- **OpenAI**
- **Anthropic**
- **OpenRouter**
- **Ollama**
- Any **OpenAI-compatible** endpoint (self-hosted or third-party)

When configuring a provider, just pick the matching API format from the dropdown in its
detail view.

<Media src="/media/1783333658458.png" caption="Choose the matching API format in AI provider settings" />

You can assign different models to different tasks (Settings → AI Copilot → Paper analysis), and
customize each task's prompt to better fit your needs:

<Media src="/media/1783333738705.png" caption="Argus lets you assign different models to different tasks" />

## What the AI can do

### Metadata extraction

When the built-in parsers can't handle it, use AI to auto-extract the title, authors,
venue, year, abstract, and more from the paper text.

### Abstract extraction

Pull the abstract straight from the paper text, or generate one with an LLM when the
built-in parser can't find it.

### Paper analysis

Generate a summary analysis of a paper. Configure the prompt and model in Settings → AI
Copilot → Paper analysis to tune the analysis to your field and workflow.

### Per-paper chat

Chat with a single paper. Argus uses the paper's text as context, so you can ask it to
explain, summarize, or clarify. Chat history is saved per paper.

<Media src="/media/ai-chat.mp4" caption="AI metadata extraction, abstract extraction, paper analysis, and per-paper chat" />

## Cost & token tracking

Every AI request records token usage and an estimated cost — if the API returns a cost,
that's used; otherwise Argus computes it from the prices you configure. Translation and
chat results show the model, input/output tokens, and cost inline, and a running log is
kept in the library folder.

<Media src="/media/1783334397708.png" caption="Argus supports per-model input/output pricing, including cache-hit and peak/off-peak rates" />

<Media src="/media/1783334422846.png" caption="View AI spending (Statistics → AI Usage)" />

### Query usage over MCP

Enable and connect the endpoint in Settings → MCP Endpoint. Codex, Claude Code and other clients can then call the read-only `get_token_usage` tool, also available to the in-app copilot. For one full day in Singapore time:

```json
{
  "start_time": "2026-10-20T00:00:00+08:00",
  "end_time": "2026-10-21T00:00:00+08:00",
  "timezone_offset_minutes": 480,
  "granularity": "day",
  "order": "asc",
  "limit": 100,
  "offset": 0
}
```

The start is inclusive and the end exclusive; omit both for all history. Optional `provider`, `model` and `source` filters match logged identifiers exactly. Discover identifiers in `by_provider`, `by_model` and `by_source` first. Timeline `granularity` accepts `hour`, `day`, `month` or `none`; buckets default to UTC, with `timezone_offset_minutes: 480` selecting UTC+8.

The result contains `summary`, all three breakdowns, `timeline` and paged `records`: input/output/total tokens, cached and uncached input, cache hit rate, recorded USD cost, and counts of records with/without costs. Cached tokens are already included in input tokens. Summaries cover every matching record, regardless of pagination. Details default to 100 records, capped at 1000; continue with `next_offset`, or set `include_records: false` for summaries only. Details default to newest first; use a fixed end time and `order: "asc"` while paging an append-only log.

Only the current library's on-disk log is queried. Timestamps represent accounting time; one record is not necessarily one user message. This tool returns recorded costs without loading model prices to estimate them: missing cost is `null`, known zero cost is `0`. Older records lacking cache data contribute zero known cache hits. Invalid or incomplete records are skipped and counted in `skipped_records`.

## Related

- [RAG & Library Q&A](/guide/rag)
- [arXiv & bioRxiv Tracking](/guide/arxiv)
