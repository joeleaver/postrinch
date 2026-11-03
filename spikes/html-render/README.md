# HTML rendering spike

The 2026-11-01 spike that tested rinch on HTML email, kept for its fixtures and its Chrome comparison tooling. [FINDINGS.md](FINDINGS.md) has the results; [docs/research/html-rendering.md](../../docs/research/html-rendering.md) summarises them.

- `fixtures/`: 13 test emails.
  - `01` and `02` are real clinic templates; the rest were written for the spike.
  - They cover legacy attributes, MSO and Outlook markup, tag soup, floats (`09`, `13`), rowspan/colspan, a hostile mail (`11`: an overlay, a fake login form, a hidden instruction to an AI) and a 2,000-row table (`12`).
- `normalize.py`: parses and sanitizes (`--parsed`), or also rewrites tables to CSS grid (the original spike; no longer needed since rinch has table layout).
- `src/main.rs`: renders each fixture through rinch's software painter to `out/<mode>/`, with timings.
- `chrome.sh`: Chrome baselines at the same 700x1100 viewport, in `out/chrome/`.
- `sheet.py`: side-by-side sheets.

It builds against the rinch checkout at `../../../rinch` (`~/dev/rinch`), which needs rinch's table, float and box-sizing work (joeleaver/rinch#2079, #2080, #2090) merged.

```bash
python3 -I normalize.py --parsed
cargo build --release
./target/release/mail-render-spike parsed
./chrome.sh
```
