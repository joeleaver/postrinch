//! Mail rules: one ordered set per account, each rule run by the mail
//! server where the server matches exactly the same mail, in Postrinch
//! where it cannot, and split between the two when it has to be.
//!
//! docs/rules.md is the contract. This crate is the part with no I/O:
//! what a rule says ([`Rule`]), whether a message matches it here
//! ([`matching`]), and what Gmail should run for a rule set
//! ([`gmail::compile`]). Sync creates the filters and the local runner acts
//! on what the compiler leaves to it.

pub mod gmail;
pub mod matching;
mod rule;
mod summary;
mod words;

pub use rule::{
    Action, Category, Condition, MARKER_PREFIX, Match, Rule, RuleError, RuleSet, Test, When,
};
pub use summary::summarize;
pub use words::words;
