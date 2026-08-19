use crate::elm::ElmSession;
use crate::error::Result;
use crate::profile::VehicleProfile;
use crate::sim::SimTransport;
use crate::telemetry::Sample;
use crate::transport::{TcpTransport, Transport};
use std::time::Duration;

/// Address of a WiCAN Pro running as its own access point. In station mode the
/// dongle takes a DHCP address instead, which the UI defaults to.
pub const AP_HOST: &str = "192.168.0.10";
pub const DEFAULT_PORT: u16 = 35_000;

pub struct Client {
    session: ElmSession<Box<dyn Transport>>,
    profile: &'static VehicleProfile,
}

impl Transport for Box<dyn Transport> {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        (**self).write_all(data)
    }
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        (**self).read(buf)
    }
}

impl Client {
    pub fn connect(
        host: &str,
        port: u16,
        profile: &'static VehicleProfile,
        timeout: Duration,
    ) -> Result<Self> {
        let transport = TcpTransport::connect((host, port), timeout)?;
        Self::with_transport(Box::new(transport), profile)
    }

    /// Connect to the built-in simulator instead of real hardware.
    pub fn connect_simulated(profile: &'static VehicleProfile) -> Result<Self> {
        Self::with_transport(Box::new(SimTransport::new()), profile)
    }

    pub fn with_transport(
        transport: Box<dyn Transport>,
        profile: &'static VehicleProfile,
    ) -> Result<Self> {
        let mut session = ElmSession::new(transport);
        session.initialize()?;
        session.apply_profile(profile)?;
        Ok(Self { session, profile })
    }

    pub fn profile(&self) -> &'static VehicleProfile {
        self.profile
    }

    /// Run every request in the profile once. A request that fails is recorded
    /// in [`Sample::stale`] rather than aborting the pass — on a real car some
    /// PIDs only answer when the vehicle is awake.
    pub fn poll(&mut self) -> Sample {
        let mut sample = Sample::default();

        for req in self.profile.requests {
            match self.session.query(req) {
                Ok(payload) => {
                    for signal in req.signals {
                        if let Some(v) = signal.decode.apply(&payload) {
                            sample.insert(signal.key, v);
                        }
                    }
                }
                Err(e) => {
                    log::debug!("{} failed: {e}", req.cmd);
                    sample.stale.push(req.cmd);
                }
            }
        }

        sample
    }

    /// Pass a command straight through to the adapter (for the terminal pane).
    pub fn raw_command(&mut self, cmd: &str) -> Result<String> {
        self.session.raw(cmd)
    }
}
