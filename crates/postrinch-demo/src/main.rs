//! `postrinch-demo <store folder>`: write the demo store into a new or
//! empty folder. Point the window at it with `XDG_DATA_HOME` set to the
//! folder's parent, so the store is `$XDG_DATA_HOME/postrinch`
//! (tools/demo-window.sh does it all).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(root), None) = (args.next(), args.next()) else {
        eprintln!("usage: postrinch-demo <store folder>   (a new or empty folder)");
        return ExitCode::FAILURE;
    };
    let root = PathBuf::from(root);
    match postrinch_demo::seed(&root, &jiff::Zoned::now()) {
        Ok(s) => {
            println!(
                "Seeded {}: {} accounts, {} conversations ({} messages), {} events, {} views, {} rules, {} social views.",
                root.display(),
                s.accounts,
                s.threads,
                s.messages,
                s.events,
                s.views,
                s.rules,
                s.social_views
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("postrinch-demo: {e}");
            ExitCode::FAILURE
        }
    }
}
