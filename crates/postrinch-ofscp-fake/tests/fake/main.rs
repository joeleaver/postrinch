//! Tests of the fake provider itself, in one binary so the shared helpers
//! in `common` are compiled once.

mod auth;
mod common;
mod federation;
mod media;
mod rest;
mod signing;
mod ws;
