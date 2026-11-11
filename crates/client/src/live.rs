//! The live event stream. Every client that follows changes uses it: the
//! app's API thread, `activity --follow` and `review wait`.
//!
//! The protocol has four parts:
//!
//! - **Hello.** The server subscribes the socket, then sends `hello` with
//!   its newest activity seq. No event after the hello is lost.
//! - **Catch-up.** Events before the hello come from `/v1/activity`, page
//!   by page. A `reload` page means some events were pruned.
//! - **Ping.** The client pings the socket. A server that sends nothing
//!   back for two pings counts as gone, even when the socket stays open.
//! - **Reload.** The server sends `reload` when its database is swapped,
//!   such as by `sync pull`.
//!
//! [`Connection`] is one socket, and [`catch_up`] reads the gap. Callers
//! with their own reconnect loop, such as the app, use these two.
//! [`follow`] adds the reconnect loop, for callers that only want events.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use futures_util::{SinkExt as _, StreamExt as _};
use llmtodo_api_types::{ActivityEvent, ActivityQuery, WsMessage};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest as _};

use crate::{Client, ClientError};

/// How long the server has to say hello after the socket opens.
const HELLO_WITHIN: Duration = Duration::from_secs(5);

/// The page size for catch-up reads.
const CATCH_UP_PAGE: u32 = 500;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What the socket says after the hello.
#[derive(Debug)]
pub enum Signal {
    /// One new activity event.
    Event(Box<ActivityEvent>),
    /// The database was swapped. Everything the caller holds may be stale.
    Reload,
}

/// One live socket, after the hello.
pub struct Connection {
    ws: Socket,
    ping: tokio::time::Interval,
    ping_every: Duration,
    heard: Instant,
    /// The server's newest activity seq when the socket opened.
    pub hello_seq: i64,
}

impl Connection {
    /// Opens the socket of `client`'s server and waits for the hello.
    /// `ping_every` sets how often [`Connection::next`] pings. An error is
    /// the reason the connection failed, for the caller to show or retry.
    pub async fn open(client: &Client, ping_every: Duration) -> Result<Self, String> {
        let mut request = client
            .ws_url()
            .into_client_request()
            .map_err(|e| e.to_string())?;
        let (name, value) = client.context_header();
        request.headers_mut().insert(
            name,
            value.parse().map_err(|e| format!("context header: {e}"))?,
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| format!("live updates: {e}"))?;
        let hello_seq = tokio::time::timeout(HELLO_WITHIN, async {
            while let Some(msg) = ws.next().await {
                if let Ok(Message::Text(text)) = msg
                    && let Ok(WsMessage::Hello { last_seq, .. }) = serde_json::from_str(&text)
                {
                    return Some(last_seq);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
        .ok_or("the server did not greet the live connection")?;
        let mut ping = tokio::time::interval(ping_every);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(Self {
            ws,
            ping,
            ping_every,
            heard: Instant::now(),
            hello_seq,
        })
    }

    /// The next signal. It pings the server while it waits. An error is
    /// the reason the connection dropped.
    ///
    /// It is safe to use in `tokio::select!`: a dropped call loses no
    /// message.
    pub async fn next(&mut self) -> Result<Signal, String> {
        loop {
            tokio::select! {
                _ = self.ping.tick() => {
                    if self.heard.elapsed() > self.ping_every * 2 {
                        return Err("the server stopped answering the live connection".into());
                    }
                    self.ws
                        .send(Message::Ping(Default::default()))
                        .await
                        .map_err(|e| format!("live updates: {e}"))?;
                }
                msg = self.ws.next() => {
                    self.heard = Instant::now();
                    let text = match msg {
                        None => return Err("the live connection closed".into()),
                        Some(Err(e)) => return Err(format!("live updates: {e}")),
                        Some(Ok(Message::Close(_))) => {
                            return Err("the server closed the live connection".into())
                        }
                        Some(Ok(Message::Text(text))) => text,
                        Some(Ok(_)) => continue,
                    };
                    match serde_json::from_str::<WsMessage>(&text) {
                        Ok(WsMessage::Event { event }) => return Ok(Signal::Event(event)),
                        Ok(WsMessage::Reload) => return Ok(Signal::Reload),
                        _ => {}
                    }
                }
            }
        }
    }
}

/// One step of a catch-up read.
#[derive(Debug)]
pub enum Caught<'a> {
    /// The next event, in seq order.
    Event(&'a ActivityEvent),
    /// Some events after the asked seq were pruned. The events that follow
    /// are the ones the server still has.
    Pruned,
}

/// Reads every event after `q.after_seq`, page by page, and hands each to
/// `each`. It moves `q.after_seq` to the last event handled. `each` returns
/// `Break` to stop, and the result is then `Break` too.
pub async fn catch_up<F>(
    client: &Client,
    q: &mut ActivityQuery,
    mut each: F,
) -> crate::Result<ControlFlow<()>>
where
    F: FnMut(Caught<'_>) -> ControlFlow<()>,
{
    loop {
        let page = client
            .activity(&ActivityQuery {
                limit: q.limit.or(Some(CATCH_UP_PAGE)),
                ..q.clone()
            })
            .await?;
        if page.reload && each(Caught::Pruned).is_break() {
            return Ok(ControlFlow::Break(()));
        }
        if page.events.is_empty() {
            return Ok(ControlFlow::Continue(()));
        }
        for e in &page.events {
            q.after_seq = Some(e.seq);
            if each(Caught::Event(e)).is_break() {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
}

/// The wait between connection tries.
#[derive(Debug, Clone)]
pub struct Backoff {
    first: Duration,
    max: Duration,
    delay: Duration,
}

impl Backoff {
    pub fn new(first: Duration, max: Duration) -> Self {
        Self {
            first,
            max,
            delay: first,
        }
    }

    /// Starts over at the first delay, after a connect.
    pub fn reset(&mut self) {
        self.delay = self.first;
    }

    /// The wait before the next try. Each call doubles the next wait, up to
    /// the limit.
    pub fn next_delay(&mut self) -> Duration {
        let wait = self.delay;
        self.delay = (self.delay * 2).min(self.max);
        wait
    }
}

/// A note for the person or agent that follows the stream.
#[derive(Debug)]
pub enum Warning {
    /// The connection dropped, and [`follow`] reconnects.
    Lost(String),
    /// The database has fewer events than the asked seq, such as after
    /// `sync pull`. The stream goes on from the server's newest event.
    Behind { asked: i64, newest: i64 },
    /// Some events after the last seq were pruned.
    Pruned,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Warning::Lost(reason) => write!(f, "live connection lost: {reason}; reconnecting"),
            Warning::Behind { asked, newest } => write!(
                f,
                "the database has fewer events than seq {asked}, such as after sync pull; following from seq {newest}"
            ),
            Warning::Pruned => write!(f, "some events after the last seq were pruned"),
        }
    }
}

/// How [`follow`] pings and retries.
#[derive(Debug, Clone)]
pub struct FollowOptions {
    pub ping_every: Duration,
    pub retry_first: Duration,
    pub retry_max: Duration,
}

impl Default for FollowOptions {
    fn default() -> Self {
        Self {
            ping_every: Duration::from_secs(15),
            retry_first: Duration::from_millis(200),
            retry_max: Duration::from_secs(5),
        }
    }
}

/// True for an error that a new connection can fix: the server is gone or
/// failed inside.
pub fn is_lost(e: &ClientError) -> bool {
    match e {
        ClientError::Unreachable(_) => true,
        ClientError::Api { status, .. } => *status >= 500,
        ClientError::Other(_) => false,
    }
}

/// Hands every event that matches `q` to `each`, in seq order, until `each`
/// returns true. The socket says when something happened, and
/// `/v1/activity` says what, with the issue filter applied.
///
/// Without `q.after_seq`, the events start after the server's newest one.
/// After a lost connection, `follow` calls `connect` for a new client,
/// which may start the server again on another port, and catches up from
/// the last event handled. It ends with an error only for a fault that a
/// reconnect cannot fix, such as an unknown issue.
pub async fn follow<C, Fut, F, W>(
    first: Client,
    mut connect: C,
    mut q: ActivityQuery,
    options: FollowOptions,
    mut warn: W,
    mut each: F,
) -> crate::Result<()>
where
    C: FnMut() -> Fut,
    Fut: std::future::Future<Output = crate::Result<Client>>,
    F: FnMut(&ActivityEvent) -> bool,
    W: FnMut(Warning),
{
    let mut backoff = Backoff::new(options.retry_first, options.retry_max);
    let mut client = first;
    loop {
        let mut reason = match follow_once(
            &client,
            &mut q,
            &options,
            &mut backoff,
            &mut warn,
            &mut each,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(Ended::Fatal(e)) => return Err(e),
            Err(Ended::Lost(m)) => m,
        };
        loop {
            warn(Warning::Lost(reason));
            tokio::time::sleep(backoff.next_delay()).await;
            match connect().await {
                Ok(c) => {
                    client = c;
                    break;
                }
                Err(e) if is_lost(&e) => reason = e.to_string(),
                Err(e) => return Err(e),
            }
        }
    }
}

/// Why one connection of [`follow`] ended.
enum Ended {
    Lost(String),
    Fatal(ClientError),
}

impl From<ClientError> for Ended {
    fn from(e: ClientError) -> Self {
        if is_lost(&e) {
            Ended::Lost(e.to_string())
        } else {
            Ended::Fatal(e)
        }
    }
}

/// One connection of [`follow`]. `Ok` means `each` asked to stop.
async fn follow_once<F, W>(
    client: &Client,
    q: &mut ActivityQuery,
    options: &FollowOptions,
    backoff: &mut Backoff,
    warn: &mut W,
    each: &mut F,
) -> Result<(), Ended>
where
    F: FnMut(&ActivityEvent) -> bool,
    W: FnMut(Warning),
{
    let mut conn = Connection::open(client, options.ping_every)
        .await
        .map_err(Ended::Lost)?;
    let newest = conn.hello_seq;
    match q.after_seq {
        None => q.after_seq = Some(newest),
        Some(asked) if asked > newest => {
            warn(Warning::Behind { asked, newest });
            q.after_seq = Some(newest);
        }
        Some(_) => {}
    }
    let mut read_gap = async |q: &mut ActivityQuery| -> Result<bool, Ended> {
        let flow = catch_up(client, q, |c| match c {
            Caught::Pruned => {
                warn(Warning::Pruned);
                ControlFlow::Continue(())
            }
            Caught::Event(e) if each(e) => ControlFlow::Break(()),
            Caught::Event(_) => ControlFlow::Continue(()),
        })
        .await?;
        Ok(flow.is_break())
    };
    if read_gap(q).await? {
        return Ok(());
    }
    backoff.reset();
    loop {
        let behind = match conn.next().await.map_err(Ended::Lost)? {
            Signal::Event(e) => Some(e.seq) > q.after_seq,
            Signal::Reload => true,
        };
        if behind && read_gap(q).await? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_limit_and_starts_over() {
        let ms = Duration::from_millis;
        let mut b = Backoff::new(ms(100), ms(350));
        assert_eq!(
            [b.next_delay(), b.next_delay(), b.next_delay()],
            [ms(100), ms(200), ms(350)]
        );
        b.reset();
        assert_eq!(b.next_delay(), ms(100));
    }
}
