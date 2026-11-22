//! Talking to a MeatPi WiCAN Pro over its TCP socket.
//!
//! The layers, bottom up:
//!
//! - [`transport`] — a byte pipe ([`transport::TcpTransport`], or [`sim`] for
//!   development away from the car).
//! - [`elm`] — ELM327 command/response framing and hex reassembly.
//! - [`profile`] — which PIDs to ask for and how to decode them.
//! - [`client`] — ties those together into [`client::Client::poll`].
//! - [`remote`] (feature `remote`) — the same [`Sample`], read from the
//!   MeatHandler server that the dongle pushes to in AutoPID mode.
//!
//! Nothing here knows about Slint, so the same crate backs the desktop and
//! Android builds, and is testable without a UI.

#[cfg(feature = "ble")]
pub mod ble;
pub mod client;
pub mod elm;
pub mod error;
pub mod profile;
#[cfg(feature = "remote-parse")]
pub mod remote;
pub mod sim;
pub mod telemetry;
pub mod transport;

pub use client::{Client, AP_HOST, DEFAULT_PORT};
pub use error::{Error, Result};
pub use profile::{VehicleProfile, KIA_EV6, PROFILES};
pub use telemetry::Sample;
