//! How Postrinch's programs (the app, `postrinch-cli`, `postrinch-mcp`)
//! filter what they log: as `RUST_LOG` says, or the program's default,
//! except for the libraries that write the raw protocol to the log, which
//! are capped at [`PROTOCOL_CAP`] whatever `RUST_LOG` says.
//!
//! Why: async-imap logs every command it sends at trace level, LOGIN's
//! password and AUTHENTICATE's base64 PLAIN or OAuth token among them,
//! and every byte it reads, mail included (`imap_stream.rs`); lettre,
//! with its `tracing` feature on, logs SMTP's lines and AUTH challenges
//! at debug level. `tracing-subscriber` hands the `log` crate's records
//! to the same filter, so a plain `RUST_LOG=trace`, or
//! `RUST_LOG=async_imap=trace`, would write passwords, tokens and mail
//! into a log the person may share. An `EnvFilter` directive cannot stop
//! that, as a more specific one in `RUST_LOG` wins over it; a second
//! filter that must also pass can (docs/imap.md, "Credentials").
//!
//! Each program uses [`filter`] on its one log layer:
//!
//! ```no_run
//! use tracing_subscriber::prelude::*;
//!
//! tracing_subscriber::registry()
//!     .with(tracing_subscriber::fmt::layer().with_filter(postrinch_log::filter("warn")))
//!     .init();
//! ```

use tracing::Subscriber;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::{FilterExt, Targets};
use tracing_subscriber::layer::Filter;
use tracing_subscriber::registry::LookupSpan;

/// The most these libraries may log: info, above where they write what
/// goes over the wire.
pub const PROTOCOL_CAP: LevelFilter = LevelFilter::INFO;

/// The libraries capped at [`PROTOCOL_CAP`], by target (a target covers
/// its modules): async-imap and its parser imap-proto, and lettre.
pub const PROTOCOL_TARGETS: [&str; 3] = ["async_imap", "imap_proto", "lettre"];

/// The filter for a program's log: `RUST_LOG` when it is set and valid,
/// else `default` (an `EnvFilter` directive such as `"warn"` or
/// `"postrinch=info"`), and in every case the protocol libraries capped.
pub fn filter<S>(default: &str) -> impl Filter<S> + Send + Sync + 'static + use<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let spec = std::env::var(EnvFilter::DEFAULT_ENV).ok();
    filter_for(spec.as_deref(), default)
}

/// [`filter`] for a given `RUST_LOG` value (`None`: unset).
fn filter_for<S>(
    spec: Option<&str>,
    default: &str,
) -> impl Filter<S> + Send + Sync + 'static + use<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let wanted = spec
        .and_then(|spec| EnvFilter::try_new(spec).ok())
        .unwrap_or_else(|| EnvFilter::new(default));
    wanted.and(protocol_cap())
}

/// Everything passes but the protocol libraries above [`PROTOCOL_CAP`].
fn protocol_cap() -> Targets {
    PROTOCOL_TARGETS.into_iter().fold(
        Targets::new().with_default(LevelFilter::TRACE),
        |cap, target| cap.with_target(target, PROTOCOL_CAP),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::prelude::*;

    use super::*;

    /// Records each event's message.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Seen {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            struct Message<'a>(&'a mut String);
            impl Visit for Message<'_> {
                fn record_debug(&mut self, _: &Field, value: &dyn std::fmt::Debug) {
                    self.0.push_str(&format!("{value:?}"));
                }
            }
            let mut message = String::new();
            event.record(&mut Message(&mut message));
            self.0.lock().unwrap().push(message);
        }
    }

    /// What gets through with `RUST_LOG` set to `spec`, of one event per
    /// target and level that matters here.
    fn passes(spec: Option<&str>, default: &str) -> Vec<String> {
        let seen = Seen::default();
        let subscriber = tracing_subscriber::registry()
            .with(seen.clone().with_filter(filter_for(spec, default)));
        tracing::subscriber::with_default(subscriber, || {
            tracing::trace!(target: "async_imap::imap_stream", "imap trace");
            tracing::debug!(target: "async_imap", "imap debug");
            tracing::info!(target: "async_imap", "imap info");
            tracing::trace!(target: "imap_proto::parser", "parser trace");
            tracing::debug!(target: "lettre::transport::smtp", "smtp debug");
            tracing::warn!(target: "lettre", "smtp warn");
            tracing::trace!(target: "postrinch_sync::imap", "own trace");
        });
        seen.0.lock().unwrap().clone()
    }

    #[test]
    fn rust_log_trace_cannot_raise_the_protocol_libraries() {
        let expected = ["imap info", "smtp warn", "own trace"];
        for spec in [
            "trace",
            "async_imap=trace,imap_proto=trace,lettre=trace,trace",
            "async_imap::imap_stream=trace,imap_proto::parser=trace,lettre::transport=trace,trace",
        ] {
            assert_eq!(passes(Some(spec), "warn"), expected, "{spec}");
        }
    }

    #[test]
    fn rust_log_still_lowers_and_chooses() {
        assert_eq!(passes(Some("warn"), "trace"), ["smtp warn"]);
        assert_eq!(passes(Some("postrinch_sync=trace"), "warn"), ["own trace"]);
        assert_eq!(passes(Some("async_imap=trace"), "warn"), ["imap info"]);
    }

    #[test]
    fn the_default_applies_when_rust_log_is_unset_or_invalid() {
        let expected = ["imap info", "smtp warn", "own trace"];
        assert_eq!(passes(None, "trace"), expected);
        assert_eq!(passes(Some("=[nonsense"), "trace"), expected);
        assert_eq!(passes(None, "warn"), ["smtp warn"]);
    }
}
