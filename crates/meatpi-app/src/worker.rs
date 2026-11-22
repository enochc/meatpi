//! The connection lives on its own thread: blocking socket reads must never
//! sit on the UI thread, and Android will kill an app that stalls its main
//! looper.
//!
//! Two kinds of link share this thread:
//! - the dongle's ELM327 socket (ELM327 protocol mode, or the simulator), polled
//!   every 700 ms;
//! - MeatHandler's `/api/latest`, polled every few seconds. This is the path
//!   when the dongle runs in AutoPID mode and pushes to MeatHandler itself.

use crate::remote_status;
use crate::secrets::{self, Login};
use meatpi_core::remote::RemoteClient;
use meatpi_core::{profile::VehicleProfile, Client, Error, Sample};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(700);
const REMOTE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
pub enum Source {
    Dongle { host: String, port: u16 },
    Simulator,
    MeatHandler {
        url: String,
        user: String,
        pass: String,
        /// Save the login to the OS secret store once it is known to work.
        remember: bool,
    },
}

pub enum Command {
    Connect(Source),
    Disconnect,
    Raw(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    Disconnected,
    Connecting,
    Live,
    Failed,
}

pub enum Event {
    Status(Status, String),
    Sample(Box<Sample>),
    Log(String),
    /// Whether a login is now saved in the secret store.
    LoginSaved(bool),
}

pub struct Worker {
    tx: Sender<Command>,
}

impl Worker {
    /// `emit` is called from the worker thread, so the UI layer is responsible
    /// for hopping back to the event loop.
    pub fn spawn<F>(profile: &'static VehicleProfile, emit: F) -> Self
    where
        F: Fn(Event) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("meatpi-link".into())
            .spawn(move || run(rx, profile, emit))
            .expect("failed to spawn link thread");
        Self { tx }
    }

    pub fn send(&self, cmd: Command) {
        // A closed channel means the worker died; the UI stays usable either
        // way, so there is nothing useful to do but note it.
        if self.tx.send(cmd).is_err() {
            log::error!("link thread is gone");
        }
    }
}

enum Link {
    Dongle(Client),
    Remote(RemoteClient),
}

impl Link {
    fn interval(&self) -> Duration {
        match self {
            Link::Dongle(_) => POLL_INTERVAL,
            Link::Remote(_) => REMOTE_POLL_INTERVAL,
        }
    }
}

/// A saved login the server now rejects (e.g. the password was changed) would
/// only fail again on the next launch, so drop it.
fn forget_login<F: Fn(Event)>(emit: &F) {
    secrets::forget();
    emit(Event::LoginSaved(false));
}

fn run<F>(rx: Receiver<Command>, profile: &'static VehicleProfile, emit: F)
where
    F: Fn(Event),
{
    let mut link: Option<Link> = None;
    let mut next_poll = Instant::now() + POLL_INTERVAL;
    // Accumulated view of the car. Merging here rather than in the UI keeps the
    // event payload self-contained, so the UI closure stays `Send`.
    let mut latest = Sample::default();
    // Remote fetches that failed in a row. A blip on cellular should not blank
    // the dashboard, so the last values stay up until this gets large.
    let mut remote_failures = 0u32;

    loop {
        let wait = next_poll.saturating_duration_since(Instant::now());

        match rx.recv_timeout(wait) {
            Ok(Command::Connect(source)) => {
                link = None;
                latest = Sample::default();
                remote_failures = 0;

                let target = match &source {
                    Source::Simulator => "simulator".to_string(),
                    Source::Dongle { host, port } => format!("{host}:{port}"),
                    Source::MeatHandler { url, .. } => url.clone(),
                };
                emit(Event::Status(
                    Status::Connecting,
                    format!("Connecting to {target}…"),
                ));
                emit(Event::Log(format!("> connecting to {target}")));

                match source {
                    Source::MeatHandler {
                        url,
                        user,
                        pass,
                        remember,
                    } => {
                        let client = RemoteClient::new(&url, &user, &pass, HTTP_TIMEOUT);
                        match client.fetch() {
                            Ok(snap) => {
                                // Only remember a login the server accepted.
                                if remember && !pass.is_empty() {
                                    secrets::save(&Login { url, user, pass });
                                    emit(Event::LoginSaved(true));
                                }
                                emit(Event::Status(Status::Live, remote_status(snap.age)));
                                emit(Event::Log(format!("< ready ({})", client.url())));
                                latest.merge_from(&snap.sample);
                                emit(Event::Sample(Box::new(latest.clone())));
                                link = Some(Link::Remote(client));
                            }
                            Err(e) => {
                                if matches!(e, Error::Http(401)) {
                                    forget_login(&emit);
                                }
                                emit(Event::Status(Status::Failed, e.to_string()));
                                emit(Event::Log(format!("< {e}")));
                            }
                        }
                        next_poll = Instant::now() + REMOTE_POLL_INTERVAL;
                    }
                    source => {
                        let attempt = match source {
                            Source::Dongle { host, port } => {
                                Client::connect(&host, port, profile, CONNECT_TIMEOUT)
                            }
                            _ => Client::connect_simulated(profile),
                        };
                        match attempt {
                            Ok(c) => {
                                emit(Event::Status(Status::Live, format!("Live · {target}")));
                                emit(Event::Log(format!("< ready ({})", profile.name)));
                                link = Some(Link::Dongle(c));
                                next_poll = Instant::now();
                            }
                            Err(e) => {
                                emit(Event::Status(Status::Failed, e.to_string()));
                                emit(Event::Log(format!("< {e}")));
                                next_poll = Instant::now() + POLL_INTERVAL;
                            }
                        }
                    }
                }
            }

            Ok(Command::Disconnect) => {
                link = None;
                latest = Sample::default();
                emit(Event::Status(Status::Disconnected, "Not connected".into()));
                emit(Event::Log("> disconnected".into()));
            }

            Ok(Command::Raw(cmd)) => match link.as_mut() {
                Some(Link::Dongle(c)) => {
                    emit(Event::Log(format!("> {cmd}")));
                    match c.raw_command(&cmd) {
                        Ok(reply) => {
                            for line in reply.lines().filter(|l| !l.trim().is_empty()) {
                                emit(Event::Log(format!("< {}", line.trim())));
                            }
                        }
                        Err(e) => emit(Event::Log(format!("< {e}"))),
                    }
                }
                Some(Link::Remote(_)) => emit(Event::Log(
                    "< the console needs a direct dongle connection (ELM327 mode)".into(),
                )),
                None => {}
            },

            Err(RecvTimeoutError::Timeout) => {
                next_poll = Instant::now() + link.as_ref().map_or(POLL_INTERVAL, Link::interval);
                match link.as_mut() {
                    Some(Link::Dongle(c)) => {
                        let sample = c.poll();
                        // Every request failing means the link is gone, not that
                        // the car is asleep — drop it so the user can retry.
                        if sample.is_empty() && !sample.stale.is_empty() {
                            emit(Event::Status(
                                Status::Failed,
                                "Adapter stopped responding".into(),
                            ));
                            emit(Event::Log("< no response to any PID".into()));
                            link = None;
                            latest = Sample::default();
                        } else {
                            latest.merge_from(&sample);
                            emit(Event::Sample(Box::new(latest.clone())));
                        }
                    }
                    Some(Link::Remote(client)) => match client.fetch() {
                        Ok(snap) => {
                            if remote_failures > 0 {
                                emit(Event::Log("< MeatHandler reachable again".into()));
                            }
                            remote_failures = 0;
                            latest.merge_from(&snap.sample);
                            emit(Event::Status(Status::Live, remote_status(snap.age)));
                            emit(Event::Sample(Box::new(latest.clone())));
                        }
                        // A bad login will not fix itself, so stop asking.
                        Err(e @ Error::Http(401)) => {
                            forget_login(&emit);
                            emit(Event::Status(Status::Failed, e.to_string()));
                            emit(Event::Log(format!("< {e}")));
                            link = None;
                            latest = Sample::default();
                        }
                        Err(e) => {
                            remote_failures += 1;
                            emit(Event::Log(format!("< {e}")));
                            emit(Event::Status(
                                Status::Live,
                                format!("MeatHandler unreachable, retrying ({remote_failures})"),
                            ));
                        }
                    },
                    None => {}
                }
            }

            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
