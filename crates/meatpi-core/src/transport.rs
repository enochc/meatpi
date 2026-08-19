use crate::error::Result;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A byte pipe to the adapter. The WiCAN Pro exposes a raw TCP socket, but
/// keeping this abstract lets the simulator (and, later, BLE) reuse the whole
/// ELM327 layer above it.
pub trait Transport: Send {
    fn write_all(&mut self, data: &[u8]) -> Result<()>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
}

pub struct TcpTransport {
    stream: TcpStream,
}

impl TcpTransport {
    /// `addr` is typically `192.168.0.10:35000` when the dongle runs as an
    /// access point, or `<dhcp-address>:35000` when it has joined your network.
    pub fn connect<A: ToSocketAddrs>(addr: A, timeout: Duration) -> Result<Self> {
        // Resolve first so we can use connect_timeout, which plain `connect`
        // does not offer; a parked car with the dongle asleep would otherwise
        // block for the OS default of ~75s.
        let mut last_err = None;
        for sockaddr in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&sockaddr, timeout) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(timeout))?;
                    stream.set_write_timeout(Some(timeout))?;
                    stream.set_nodelay(true)?;
                    return Ok(Self { stream });
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err
            .unwrap_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "no address resolved")
            })
            .into())
    }
}

impl Transport for TcpTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        Write::write_all(&mut self.stream, data)?;
        self.stream.flush()?;
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        Ok(Read::read(&mut self.stream, buf)?)
    }
}
