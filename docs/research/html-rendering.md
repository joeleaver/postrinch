# Rendering HTML mail in rinch

From the rendering spike of 2026-11-01 (local folder `~/dev/mail-render-spike`, whose `FINDINGS.md` has the screenshots and timings) and the rinch work that followed it.

## What the spike found

Twelve fixtures were rendered at 700x1100 by headless Chrome (the baseline), by rinch as it was (`set_inner_html` on the raw body), and by rinch after an app-side pipeline: an HTML5 parse, a sanitizer, legacy attributes turned into CSS, and tables rewritten to CSS grid.

- **rinch as it was failed most mail:** no table layout, no legacy attributes (`bgcolor`, `align`, `cellpadding`), a parser that is not the HTML5 algorithm (implied end tags, most named entities), and no floats.
- **The pipeline got 9 of the 10 comparable fixtures close to Chrome.** The exception was floated images with text wrapping, which no pipeline can fake.
- **A sanitizer is required anyway:** the hostile fixture's overlay, fake login form and iframe all rendered in raw rinch.
- **Speed:** typical mail lays out and paints in 5-14 ms; a 2,000-row table took about 0.65 s after the rewrite.
- **Limits:** only 2 of the 12 fixtures were real mail, both from one sender; software renderer only; no scrolling, selection or link clicks tested; remote images untested.

## What changed in rinch since

- **Tables and legacy attributes** ([rinch#2079](https://github.com/joeleaver/rinch/pull/2079)): real table layout on Taffy's grid (rowspan/colspan, column widths from content, `vertical-align`, collapsed borders, row and group backgrounds) and `bgcolor`/`align`/`width`/`cellpadding`/`border` and friends as presentational hints. The table-to-grid rewrite is no longer needed: Postrinch's pipeline only parses and sanitizes.
- **Floats** ([rinch#2080](https://github.com/joeleaver/rinch/pull/2080), stacked on #2079): `float`, `clear`, `align=left|right` on images and tables. In progress: lines wrapping around a float one by one and flowing back under it, as Chrome draws them (a Taffy fork hands paragraphs the float context; parley places each line). A float in the middle of a line still splits that line.

## What Postrinch's pipeline does

1. MIME: pick the best part (`text/html`, else `text/plain` shown as text), resolve `cid:` images from attachments.
2. Parse with html5ever (the HTML5 algorithm, as mail clients and browsers do).
3. Sanitize with an allow-list: drop scripts, forms, frames, objects, event attributes, `javascript:` URLs, `position: fixed` and the like; keep `<style>` but scope it to the message.
4. Remote content policy: off by default; images and fonts load per sender or per message on request, without cookies.
5. Hand the document to rinch.
6. Flag text a person cannot see (white on white, 1px, hidden), for the agent's sake (see DESIGN.md, Safety).

## Still to learn

- **A real corpus.** A few hundred real `.eml` files through this pipeline, before deciding how much rides on "open in browser".
- **Quirks mode.** Mail without a doctype renders in Chrome's quirks mode, which rinch does not have; the remaining fixture differences were mostly that.
- **The GPU renderer**, scrolling, selection and link handling in a real window.
