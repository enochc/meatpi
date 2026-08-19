//! A fake WiCAN Pro, so the UI can be developed and demoed without sitting in
//! the car. It speaks just enough ELM327 to satisfy [`crate::elm::ElmSession`].

use crate::error::Result;
use crate::transport::Transport;
use std::collections::VecDeque;
use std::time::Instant;

pub struct SimTransport {
    pending: VecDeque<u8>,
    started: Instant,
    header: String,
}

impl Default for SimTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl SimTransport {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            started: Instant::now(),
            header: "7DF".into(),
        }
    }

    /// Drive the synthetic values so the dashboard visibly moves: a slow
    /// discharge with a sinusoidal load, as if driving.
    fn drift(&self) -> (f64, f64, f64) {
        let t = self.started.elapsed().as_secs_f64();
        let soc = (78.0 - t / 60.0).clamp(5.0, 100.0);
        // An 800 V pack pulls far less current than a 400 V one for the same
        // power, so keep the simulated numbers in a believable range.
        let current = 60.0 + 40.0 * (t / 7.0).sin();
        let speed = (55.0 + 25.0 * (t / 11.0).sin()).clamp(0.0, 130.0);
        (soc, current, speed)
    }

    fn respond(&mut self, cmd: &str) {
        let cmd = cmd.trim().to_uppercase();
        let (soc, current, speed) = self.drift();

        let body = if let Some(rest) = cmd.strip_prefix("ATSH") {
            self.header = rest.to_string();
            "OK".to_string()
        } else if cmd.starts_with("AT") {
            if cmd == "ATZ" {
                "ELM327 v2.2".to_string()
            } else {
                "OK".to_string()
            }
        } else if cmd == "220101" {
            let mut p = vec![0u8; 32];
            p[4] = (soc * 2.0) as u8;
            put_u16(&mut p, 5, 6_000); // 60.00 kW regen limit
            put_u16(&mut p, 7, 18_000); // 180.00 kW discharge limit
            put_i16(&mut p, 10, (current * 10.0) as i16);
            put_u16(&mut p, 12, 7_477); // 747.7 V — the EV6 is an 800 V pack
            p[14] = 35;
            p[15] = 33;
            p[23] = 194; // highest cell, 3.88 V
            p[25] = 194; // lowest cell, 3.88 V
            p[29] = 141; // 14.1 V
            frame_response(&[0x62, 0x01, 0x01], &p)
        } else if cmd == "220105" {
            let mut p = vec![0u8; 34];
            p[20] = 2; // 0.04 V spread
            p[23] = 22;
            put_u16(&mut p, 25, 989); // 98.9 % SOH
            p[31] = (soc * 2.0) as u8;
            frame_response(&[0x62, 0x01, 0x05], &p)
        } else if cmd == "010D" {
            frame_response(&[0x41, 0x0D], &[speed as u8])
        } else if cmd == "0146" {
            frame_response(&[0x41, 0x46], &[40 + 19])
        } else {
            "NO DATA".to_string()
        };

        for b in body.bytes() {
            self.pending.push_back(b);
        }
        self.pending.extend(b"\r\r>".iter().copied());
    }
}

fn put_u16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn put_i16(buf: &mut [u8], at: usize, v: i16) {
    buf[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

/// Render bytes the way an ELM327 does for a multi-frame response: a total
/// length line, then seven data bytes per line behind a `n:` index. Matches
/// what the real WiCAN Pro emits.
fn frame_response(marker: &[u8], payload: &[u8]) -> String {
    let all: Vec<u8> = marker.iter().chain(payload.iter()).copied().collect();
    if all.len() <= 7 {
        return all
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
    }
    let lines: Vec<String> = all
        .chunks(7)
        .enumerate()
        .map(|(i, chunk)| {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            format!("{:X}: {}", i, hex.join(" "))
        })
        .collect();
    format!("{:03X}\r{}", all.len(), lines.join("\r"))
}

impl Transport for SimTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let text = String::from_utf8_lossy(data);
        for cmd in text.split('\r').filter(|c| !c.trim().is_empty()) {
            self.respond(cmd);
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = buf.len().min(self.pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.pending.pop_front().unwrap();
        }
        Ok(n)
    }
}
