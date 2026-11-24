//! IDLE (RFC 2177): the client waits, and the server tells it at once
//! about changes other sessions or the test make to its folder.

use std::io;

use super::config::Extension;
use super::response::Args;
use super::session::{Next, Reply, Session};

impl Session {
    pub(crate) async fn idle(&mut self, args: Args) -> io::Result<(Reply, Next)> {
        if !self.shared.lock().has(Extension::Idle) {
            return Ok((Reply::Bad("Unknown command IDLE".into()), Next::Continue));
        }
        if self.user.is_none() {
            return Ok((
                Reply::Bad("IDLE needs a signed-in session".into()),
                Next::Continue,
            ));
        }
        if let Err(e) = args.end() {
            return Ok((Reply::Bad(e), Next::Continue));
        }
        self.out.extend_from_slice(b"+ idling\r\n");
        loop {
            if self.killed() {
                return Ok((Reply::Bad("Dropped".into()), Next::Drop));
            }
            if !self.sync_selected(true) {
                self.out
                    .extend_from_slice(b"* BYE The selected mailbox is gone\r\n");
                self.send_pending().await?;
                return Ok((Reply::Bad("Mailbox gone".into()), Next::Drop));
            }
            self.send_pending().await?;
            if let Some(line) = self.conn.take_line() {
                return Ok(if line.eq_ignore_ascii_case(b"DONE") {
                    (Reply::Ok("IDLE terminated".into()), Next::Continue)
                } else {
                    (Reply::Bad("Expected DONE".into()), Next::Continue)
                });
            }
            tokio::select! {
                more = self.conn.fill() => if !more? {
                    return Ok((Reply::Bad("Client left".into()), Next::Drop));
                },
                changed = self.changes.changed() => if changed.is_err() {
                    return Ok((Reply::Bad("Server stopped".into()), Next::Drop));
                },
            }
        }
    }
}
