//! The connection lives on its own thread: blocking socket reads must never
//! sit on the UI thread, and Android will kill an app that stalls its main
//! looper.

use meatpi_core::{profile::VehicleProfile, Client, Sample};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(700);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub enum Command {
    Connect {
        host: String,
        port: u16,
        simulate: bool,
    },
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

fn run<F>(rx: Receiver<Command>, profile: &'static VehicleProfile, emit: F)
where
    F: Fn(Event),
{
    let mut client: Option<Client> = None;
    let mut next_poll = Instant::now() + POLL_INTERVAL;
    // Accumulated view of the car. Merging here rather than in the UI keeps the
    // event payload self-contained, so the UI closure stays `Send`.
    let mut latest = Sample::default();

    loop {
        let wait = next_poll.saturating_duration_since(Instant::now());

        match rx.recv_timeout(wait) {
            Ok(Command::Connect {
                host,
                port,
                simulate,
            }) => {
                client = None;
                latest = Sample::default();
                let target = if simulate {
                    "simulator".to_string()
                } else {
                    format!("{host}:{port}")
                };
                emit(Event::Status(
                    Status::Connecting,
                    format!("Connecting to {target}…"),
                ));
                emit(Event::Log(format!("> connecting to {target}")));

                let attempt = if simulate {
                    Client::connect_simulated(profile)
                } else {
                    Client::connect(&host, port, profile, CONNECT_TIMEOUT)
                };

                match attempt {
                    Ok(c) => {
                        emit(Event::Status(Status::Live, format!("Live · {target}")));
                        emit(Event::Log(format!("< ready ({})", profile.name)));
                        client = Some(c);
                        next_poll = Instant::now();
                    }
                    Err(e) => {
                        emit(Event::Status(Status::Failed, e.to_string()));
                        emit(Event::Log(format!("< {e}")));
                        next_poll = Instant::now() + POLL_INTERVAL;
                    }
                }
            }

            Ok(Command::Disconnect) => {
                client = None;
                latest = Sample::default();
                emit(Event::Status(Status::Disconnected, "Not connected".into()));
                emit(Event::Log("> disconnected".into()));
            }

            Ok(Command::Raw(cmd)) => {
                if let Some(c) = client.as_mut() {
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
            }

            Err(RecvTimeoutError::Timeout) => {
                next_poll = Instant::now() + POLL_INTERVAL;
                if let Some(c) = client.as_mut() {
                    let sample = c.poll();
                    // Every request failing means the link is gone, not that
                    // the car is asleep — drop it so the user can retry.
                    if sample.is_empty() && !sample.stale.is_empty() {
                        emit(Event::Status(
                            Status::Failed,
                            "Adapter stopped responding".into(),
                        ));
                        emit(Event::Log("< no response to any PID".into()));
                        client = None;
                        latest = Sample::default();
                    } else {
                        latest.merge_from(&sample);
                        emit(Event::Sample(Box::new(latest.clone())));
                    }
                }
            }

            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
