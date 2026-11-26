# Source of these fixtures

Copied unchanged from the OFSCP specification repository, `tests/`, at
lost-conn/ofscp commit `37b0cc2` (main, after spec PR #28). MIT licensed,
copyright lost-conn.

Left out, because this crate does not model them yet: calls (`call-*`,
`ws-call-*`; a later wave, docs/ofscp.md decision 2), Web Push (`push-*`) and
notification webhooks (`notifications-webhook*`, `notifications-endpoints*`,
`notifications-delivery*`).

The spec's README says its JSON Schemas and samples were written with an LLM
and are not yet reviewed, so a disagreement with the prose goes to the spec
process (docs/ofscp.md, "Working with the spec"), not into a private fix here.

To update: copy the same files from a newer spec commit, change the commit
above, and run `cargo test -p postrinch-ofscp`.
