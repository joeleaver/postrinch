# HTML email in rinch: spike findings (2026-11-01)

**Verdict: rinch is viable for HTML email without adding table layout, but not with `set_inner_html` alone.**
An app-side pipeline gets 9 of the 10 comparable fixtures close to Chrome. The exception needs float support in rinch.

(`out/` is not kept in the repository: the commands in the README regenerate every image named here.)

Overview image: [out/overview_chrome_vs_norm.png](out/overview_chrome_vs_norm.png) (each pair is Chrome on the left, normalized rinch on the right).
Per-fixture sheets: `out/sheet_<fixture>.png` (Chrome, raw, normalized).

## Update: rinch table layout branch (same day)

The `table-layout` branch of rinch adds real table layout and legacy attribute hints, so the table-to-grid rewrite is no longer needed. The new `parsed` mode (`python3 -I normalize.py --parsed`, then `mail-render-spike parsed`) only parses and sanitizes, and `Cargo.toml` here currently points at the `floats` worktree (rinch PR #2080, stacked on #2079), which also adds floats. Sheets: `out/tablelayout_overview.png`. Remaining differences on the fixtures come from no-doctype mail rendering in Chrome's quirks mode, which rinch does not have, and text that Chrome wraps back under a float (fixture 09), which rinch keeps in the column beside it. Timings on an idle machine: typical mail 5-22ms; the 2000-row table about 0.86s.

**Update, 2026-11-02 (floats and box-sizing).** The work below became a stack of three rinch PRs (`Cargo.toml` now points at `~/dev/rinch`, which needs them merged):
- #2079: tables.
- #2080: floats.
  - Lines wrap around floats line by line.
  - A float in a paragraph is placed at the top of the line it stands in.
  - A box that contains only floats keeps its bottom padding.
  - This needs rinch's Taffy fork, `joeleaver/taffy-fork`.
- #2090: rinch now reads `box-sizing`, which it used to ignore (#1278).

Results:
- **Fixture 09 (floats)** now matches Chrome box for box, with the same line breaks: `out/09_chrome_vs_rinch.png`.
- **New fixture 13 (floats inside paragraphs)** matches too: `out/13_chrome_vs_rinch.png`.
- **The harness** now puts the mail's `<body style>` on the real body. Before, it went on a wrapper, so the body kept its default 8px margin.
- **Remaining differences** (`out/boxsizing_overview.png`) are the quirks-mode ones in mail with no doctype: 03, 05, 06, 07 and 10.

The rest of this file describes the original spike.

## What was compared

Each fixture was rendered at 700x1100 in three ways:

- **chrome**: headless Chrome, the baseline.
- **raw**: rinch today. `<style>` is split out, the body goes through `load_css` and `set_inner_html`.
- **norm**: `normalize.py`, then the same rinch path. It does an HTML5 parse (html5lib standing in for html5ever), sanitizes, turns legacy attributes into CSS, and rewrites `<table>` to CSS grid. rinch itself is unchanged.

## Results

Judged by eye from the screenshots.

| Fixture | Source | raw | norm |
|---|---|---|---|
| 01 password reset | real clinic template | Poor: card and button background lost | Good |
| 02 invoice | real clinic template | Broken: rows run together | Good; column widths differ |
| 03 receipt, legacy attributes | written for spike | Broken | Good |
| 04 two-column newsletter | written for spike | Broken: columns stack, header overlaps | Very close |
| 05 hybrid/fluid with MSO comments | written for spike | Broken: white card lost, text unreadable | Good |
| 06 legacy tag soup | written for spike | Broken: no table, entities shown literally | Good; small differences |
| 07 Gmail reply | written for spike | Good | Good |
| 08 Outlook/Word | written for spike | Table broken, rest good | Very close |
| 09 floats, `align` on images | written for spike | Wrong: no float or wrapping | **Still wrong** |
| 10 data table, rowspan/colspan | written for spike | Broken | Good; column widths differ |
| 11 hostile | written for spike | **Unsafe**: red overlay, fake login form, iframe all shown | Safe; those are gone |
| 12 2000-row table | generated | Timing only, not checked visually | Timing only, not checked visually |

Chrome produced no baseline for 11, most likely because the page's scripts blocked the screenshot. That is why 10 fixtures are comparable.

## What is missing in rinch

Each of these was seen in a rendered result and traced to code:

1. **Table layout.** There is no table formatting context. `table` becomes block and `tr`/`td` stay inline, unless the element's display is set explicitly.
2. **Legacy attributes.** `bgcolor`, `align`, and `cellpadding` map to nothing (`stylo_impl.rs`, `synthesize_presentational_hints_for_legacy_attributes`).
3. **HTML parsing.** `html_parser.rs` is a simple fragment parser, not the HTML5 algorithm. Implied end tags fail (`<LI>` runs, `<TD>` without a close), and only some entities decode (`&mdash;`, `&pound;`, `&copy;` printed literally).
4. **Floats.** They are not supported. This is the one gap the pipeline cannot work around.

Gaps 1-3 are all handled by normalizing before the DOM. A real parser and sanitizer are needed anyway, because mail is hostile input.

## Performance

Software renderer, release build, one run each. Figures are milliseconds on this machine.

| Fixture | raw DOM | raw layout+paint | norm DOM | norm layout+paint |
|---|---|---|---|---|
| Typical email (01-11) | under 3 | 4-12 | under 1 | 5-14 |
| 2000-row table | 47 | 236 | 136 | 520 |

Typical mail is well inside a frame. The 2000-row case takes about 0.65s normalized, and the rewrite grows that HTML 6.7x (203KB to 1.37MB, mostly inline style). Compact the output with shared classes before relying on this for long mail.

These timings exclude `normalize.py` itself, which would be Rust in a real client.

## Recommendation

1. Build the pipeline in Rust: html5ever, sanitizer, attribute mapping, table-to-grid.
2. Keep the table tags and use `display: contents` on rows. Replacing them with divs broke author selectors such as `td` and `.table td`, which is what first damaged the invoice.
3. Add float support to rinch. Treat real table layout as a later fidelity upgrade, not a blocker.
4. Offer "open in browser" as a fallback for mail that still renders badly.

## Limits of this spike

- Only 2 of the 12 fixtures are real, and both come from one sender. The rest are written to match common patterns, so 9 of 10 is **not** an estimate for a real inbox. Run a sample of your own `.eml` files through this before committing; that needs a small MIME-extraction step, which does not exist yet.
- Tested in the software painter only. The GPU path was not rendered.
- Static screenshots only. Scrolling, text selection, and link clicks were not tested.
- Remote images are untested. `normalize.py` strips them. rinch-dom has no HTTP client of its own and loads through a pluggable `ImageLoader`, but I did not confirm what the default loader does.
- The hidden prompt-injection line in fixture 11 (white, 1px text) does not show visually, but it stays in the normalized DOM. A renderer's sanitizer does not protect the agent; the agent-side safeguards still have to.

## Reproduce

```bash
python3 -I normalize.py
cargo build --release --offline
./target/release/mail-render-spike raw
./target/release/mail-render-spike norm
./chrome.sh
python3 sheet.py chrome raw norm
```
