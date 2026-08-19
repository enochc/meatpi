use crate::error::{Error, Result};
use crate::transport::Transport;
use std::time::{Duration, Instant};

/// ELM327 signals end-of-response with this prompt character.
const PROMPT: u8 = b'>';

pub struct ElmSession<T: Transport> {
    transport: T,
    /// Last header set with `ATSH`, so we skip redundant round trips when
    /// polling several PIDs from the same ECU.
    current_header: Option<String>,
    read_timeout: Duration,
}

impl<T: Transport> ElmSession<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            current_header: None,
            read_timeout: Duration::from_millis(2000),
        }
    }

    pub fn set_read_timeout(&mut self, timeout: Duration) {
        self.read_timeout = timeout;
    }

    /// Send a command and return the raw text the adapter replied with,
    /// prompt stripped.
    pub fn command(&mut self, cmd: &str) -> Result<String> {
        let line = format!("{cmd}\r");
        self.transport.write_all(line.as_bytes())?;
        self.read_until_prompt()
    }

    fn read_until_prompt(&mut self) -> Result<String> {
        let mut out = Vec::new();
        let mut buf = [0u8; 512];
        let deadline = Instant::now() + self.read_timeout;

        loop {
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            let n = self.transport.read(&mut buf)?;
            if n == 0 {
                return Err(Error::Adapter("connection closed by adapter".into()));
            }
            out.extend_from_slice(&buf[..n]);
            if out.contains(&PROMPT) {
                break;
            }
        }

        let text = String::from_utf8_lossy(&out).replace('\r', "\n");
        Ok(text.trim_end_matches(['>', '\n', ' ']).to_string())
    }

    /// Reset the adapter and put it in the mode this app expects: echo off,
    /// spaces on (so the line parser can tell a CAN id from a data byte),
    /// headers off, CAN auto-formatting on so the adapter handles ISO-TP
    /// flow control for us.
    pub fn initialize(&mut self) -> Result<()> {
        // ATZ reboots the interpreter and can take up to a second.
        let saved = self.read_timeout;
        self.read_timeout = saved.max(Duration::from_millis(3000));
        let _ = self.command("ATZ")?;
        self.read_timeout = saved;

        for cmd in ["ATE0", "ATL0", "ATS1", "ATH0", "ATCAF1", "ATST32"] {
            let reply = self.command(cmd)?;
            if reply.to_uppercase().contains('?') {
                return Err(Error::Adapter(format!("{cmd} rejected: {reply}")));
            }
        }
        self.current_header = None;
        Ok(())
    }

    /// Apply a vehicle profile's protocol and flow-control setup.
    pub fn apply_profile(&mut self, profile: &crate::profile::VehicleProfile) -> Result<()> {
        for cmd in profile.init_commands {
            self.command(cmd)?;
        }
        self.current_header = None;
        Ok(())
    }

    /// Run one request and return the decoded payload that follows the
    /// positive-response marker.
    pub fn query(&mut self, req: &crate::profile::PidRequest) -> Result<Vec<u8>> {
        if self.current_header.as_deref() != Some(req.header) {
            let reply = self.command(&format!("ATSH{}", req.header))?;
            if !reply.to_uppercase().contains("OK") {
                return Err(Error::Adapter(format!(
                    "could not set header {}: {reply}",
                    req.header
                )));
            }
            self.current_header = Some(req.header.to_string());
        }

        // The first request after protocol setup often returns only the first
        // ISO-TP frame, so allow one retry when the reply is short.
        let mut last = Err(Error::NoData);
        for _ in 0..2 {
            let raw = self.command(req.cmd)?;
            let upper = raw.to_uppercase();
            if upper.contains("NO DATA") || upper.contains("UNABLE TO CONNECT") {
                return Err(Error::NoData);
            }
            if upper.contains("CAN ERROR")
                || upper.contains("BUS INIT")
                || upper.contains("STOPPED")
            {
                return Err(Error::Adapter(raw.trim().to_string()));
            }

            let bytes = assemble_hex(&raw);
            if let Some(expected) = declared_length(&raw) {
                if bytes.len() < expected {
                    log::debug!(
                        "{} truncated: {} of {expected} bytes, retrying",
                        req.cmd,
                        bytes.len()
                    );
                    last = Err(Error::Adapter(format!(
                        "{} truncated: {} of {expected} bytes",
                        req.cmd,
                        bytes.len()
                    )));
                    continue;
                }
            }
            return payload_after_marker(&bytes, req.marker).ok_or(Error::NoData);
        }
        last
    }

    /// Escape hatch for the terminal pane in the UI.
    pub fn raw(&mut self, cmd: &str) -> Result<String> {
        self.current_header = None; // the user may have changed it themselves
        self.command(cmd)
    }
}

/// Flatten an ELM327 reply into the bytes it carries.
///
/// The adapter has several output shapes depending on header and formatting
/// settings, and multi-frame ISO-TP responses get line-index prefixes:
///
/// ```text
/// 7EC 10 3E 62 01 01 FF     headers on, single line
/// 0: 62 01 01 FF F7 E7      headers off, multi-frame
/// 7EC0:6201013EFFF7E7       some firmware packs it tight
/// ```
///
/// Rather than guess the shape, drop anything before a `:` (the line index and
/// any header the adapter printed with it), then skip tokens that are the
/// wrong width to be a data byte.
fn assemble_hex(raw: &str) -> Vec<u8> {
    let mut out = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_uppercase();
        // Status chatter the adapter interleaves with real responses.
        if upper.starts_with("SEARCHING") || upper == "OK" || upper.starts_with("BUS") {
            continue;
        }

        let body = match line.split_once(':') {
            Some((_, rest)) => rest,
            None => line,
        };

        let tokens: Vec<&str> = body.split_whitespace().collect();
        if tokens.len() == 1 {
            // No spaces: assume a packed hex run.
            push_hex_run(tokens[0], &mut out);
        } else {
            for token in tokens {
                // 3 nibbles = 11-bit CAN id, 8 = 29-bit id. Neither is payload.
                if token.len() == 3 || token.len() == 8 {
                    continue;
                }
                push_hex_run(token, &mut out);
            }
        }
    }

    out
}

/// Before a multi-frame response the adapter prints the total byte count on a
/// line of its own, e.g. `03E` ahead of `0: 62 01 01 …`. Comparing it against
/// what we actually assembled is how truncation is caught.
fn declared_length(raw: &str) -> Option<usize> {
    raw.lines()
        .map(str::trim)
        .find(|line| line.len() == 3 && line.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|line| usize::from_str_radix(line, 16).ok())
}

fn push_hex_run(token: &str, out: &mut Vec<u8>) {
    let digits: Vec<u8> = token.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    for pair in digits.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16).unwrap() as u8;
        let lo = (pair[1] as char).to_digit(16).unwrap() as u8;
        out.push(hi << 4 | lo);
    }
}

/// Find the positive-response marker (e.g. `62 01 01` for request `220101`)
/// and return everything after it. Searching for the marker rather than
/// assuming a fixed offset keeps us immune to whether the adapter printed
/// headers or ISO-TP length bytes.
fn payload_after_marker(bytes: &[u8], marker: &[u8]) -> Option<Vec<u8>> {
    if marker.is_empty() || bytes.len() < marker.len() {
        return None;
    }
    bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .map(|idx| bytes[idx + marker.len()..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_off_multiframe() {
        let raw = "0: 62 01 01 FF F7 E7\n1: FF 99 3A 25 25 25 25";
        let bytes = assemble_hex(raw);
        let payload = payload_after_marker(&bytes, &[0x62, 0x01, 0x01]).unwrap();
        assert_eq!(payload[0], 0xFF);
        assert_eq!(payload[3], 0xFF);
        assert_eq!(payload[4], 0x99);
    }

    #[test]
    fn parses_headers_on_single_line() {
        let raw = "7EC 03 41 0D 5A";
        let bytes = assemble_hex(raw);
        // The 3-nibble CAN id is skipped; the ISO-TP length byte 0x03 stays in
        // the stream but sits before the marker, so it does not shift anything.
        let payload = payload_after_marker(&bytes, &[0x41, 0x0D]).unwrap();
        assert_eq!(payload[0], 0x5A);
    }

    #[test]
    fn parses_packed_hex_without_spaces() {
        let raw = "7EC0:6201013EFFF7E7";
        let bytes = assemble_hex(raw);
        let payload = payload_after_marker(&bytes, &[0x62, 0x01, 0x01]).unwrap();
        assert_eq!(payload[0], 0x3E);
    }

    #[test]
    fn ignores_searching_chatter() {
        let raw = "SEARCHING...\n41 0D 32";
        let bytes = assemble_hex(raw);
        let payload = payload_after_marker(&bytes, &[0x41, 0x0D]).unwrap();
        assert_eq!(payload[0], 0x32);
    }

    /// Captured from the car: a complete 220101 reply, 0x3E = 62 bytes.
    const REAL_220101: &str = "03E\n0: 62 01 01 EF FB E7\n1: EF 8E 00 00 00 00 00\n\
                               2: 00 10 1D 35 23 21 23\n3: 22 22 21 22 00 4D C2\n\
                               4: 99 C2 2F 00 00 8C 00\n5: 00 3D 62 00 00 3B 0C\n\
                               6: 00 00 2E 1F 00 00 2B\n7: 1E 00 26 66 0C 00 02\n\
                               8: E8 00 00 00 00 06 7B";

    #[test]
    fn reads_declared_length_and_accepts_a_complete_reply() {
        assert_eq!(declared_length(REAL_220101), Some(0x3E));
        let bytes = assemble_hex(REAL_220101);
        assert!(bytes.len() >= 0x3E, "assembled {} bytes", bytes.len());

        // Spot-check the values verified against the car.
        let p = payload_after_marker(&bytes, &[0x62, 0x01, 0x01]).unwrap();
        assert_eq!(p[4], 0x8E); // SOC 71 %
        assert_eq!(u16::from_be_bytes([p[12], p[13]]), 7477); // 747.7 V
        assert_eq!(p[29], 0x8C); // 14.0 V accessory battery
    }

    #[test]
    fn detects_a_truncated_multiframe_reply() {
        // What the adapter actually returned on the first request: length says
        // 62 bytes, only the first frame arrived.
        let truncated = "03E\n0: 62 01 01 EF FB E7";
        let expected = declared_length(truncated).unwrap();
        assert_eq!(expected, 0x3E);
        assert!(assemble_hex(truncated).len() < expected);
    }

    #[test]
    fn single_frame_replies_have_no_declared_length() {
        assert_eq!(declared_length("41 0D 5A"), None);
        assert_eq!(declared_length("NO DATA"), None);
    }

    #[test]
    fn missing_marker_is_none() {
        let bytes = assemble_hex("7F 22 31");
        assert!(payload_after_marker(&bytes, &[0x62, 0x01, 0x01]).is_none());
    }
}
