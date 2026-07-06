# arXiv & bioRxiv Tracking

Argus can keep an eye on new preprints for you — fetching, filtering, analyzing, and
triaging them so you spend your time reading the right papers instead of hunting for them.

## Configuration

Before using arXiv / bioRxiv tracking, configure it in Settings: the categories to crawl,
keywords you care about, crawl frequency, crawl time, look-back days, whether to enable
automatic AI analysis, and the model and prompt for that analysis. Once configured, Argus
fetches new papers from arXiv / bioRxiv on schedule and drops them into a daily inbox.

<Media src="/media/1783338672276.png" caption="The daily inbox with AI relevance scores and one-click import" />

## Daily inbox

Argus fetches new papers from **arXiv** and **bioRxiv** on schedule and drops them into a
daily **inbox**. There you can:

- Browse new recommendations grouped by date
- Mark papers read/unread and rate them
- Add useful papers straight to your library
- Bulk-delete inbox entries by date range

## Scheduled fetching

A background scheduler fetches on your configured cadence and catches up on missed days
based on your look-back setting. Even if the arXiv window isn't open, the inbox stays up to
date as long as Argus is running.

<Media src="/media/1783338965093.png" caption="You can also trigger an arXiv fetch manually" />

## AI relevance filtering

Argus can automatically analyze the arXiv papers it collects and, based on your keywords,
surface the ones you're likely to care about — freeing you from the grind of hunting for
papers. It filters by a **threshold you set**, which you can use to tune how strict the
filtering is.

<Media src="/media/arxiv-ai-filter.mp4" caption="Argus automatically surfaces papers you care about based on your keywords" />

During AI analysis, papers below the threshold are deleted outright, while the ones you're
interested in are tagged and kept. AI analysis handles up to 10 papers at once, greatly
speeding things up.

## Related

- [Quick Start](/guide/quick-start)
- [AI Workflows](/guide/ai)
