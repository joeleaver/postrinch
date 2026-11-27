//! A live account's outbox, sent by itself: a pass when the account goes
//! live, when told there is more, when one of its sessions catches up
//! after connecting (the network is back), and again with backoff while a
//! change fails, no sooner than a provider's `Retry-After`.

use std::time::Duration;

use postrinch_ofscp::ofscp_core::Actor;
use postrinch_store::social_account_key;
use postrinch_sync_core::Backoff;
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::live::Control;
use crate::live::LiveEvent;
use crate::{Social, SocialError};

/// Send `actor`'s outbox until `nudges` closes: a pass now, and another
/// after each nudge. When the account can no longer sign, or is gone, it
/// tells the account's run by `control` (which ends the live account,
/// saying why) and ends.
pub(crate) async fn run(
    social: Social,
    actor: Actor,
    control: mpsc::UnboundedSender<Control>,
    mut nudges: mpsc::UnboundedReceiver<()>,
) {
    let key = social_account_key(&actor.to_string());
    let options = &social.live.options;
    let mut backoff = Backoff::with(options.first_backoff, options.max_backoff);
    let mut events = social.live_events();
    loop {
        // Nudges sent before this pass are answered by it; one sent while
        // it runs brings another.
        while nudges.try_recv().is_ok() {}
        let retry = match social.send_outbox_of(&actor).await {
            Ok(sent) if sent.failed == 0 => {
                backoff.reset();
                None
            }
            Ok(sent) => Some(wait(&mut backoff, sent.retry_after)),
            Err(SocialError::Unusable { state, .. }) => {
                let _ = control.send(Control::Unusable(state));
                return;
            }
            Err(SocialError::NoAccount(_)) => {
                let _ = control.send(Control::Stop);
                return;
            }
            Err(error) => {
                tracing::warn!(%actor, %error, "the outbox could not be sent");
                Some(wait(&mut backoff, None))
            }
        };
        let deadline = retry.map(|r| Instant::now() + r);
        loop {
            tokio::select! {
                nudge = nudges.recv() => match nudge {
                    Some(()) => break,
                    None => return,
                },
                event = events.recv() => match event {
                    Ok(LiveEvent::CaughtUp { account, .. })
                        if social_account_key(&account) == key => break,
                    Ok(_) => {}
                    // Some events were missed, a catch-up among them
                    // perhaps: a pass costs little.
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                () = sleep_until(deadline) => break,
            }
        }
    }
}

/// The wait before trying again: the backoff's next, or longer when the
/// provider asked.
fn wait(backoff: &mut Backoff, retry_after: Option<Duration>) -> Duration {
    backoff.next_delay().max(retry_after.unwrap_or_default())
}

/// Sleep until `deadline`; forever without one.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
