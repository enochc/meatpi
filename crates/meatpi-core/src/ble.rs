//! BLE transport for the WiCAN Pro.
//!
//! The dongle exposes a UART-style pair of characteristics; feeding them
//! through [`Transport`] means [`crate::elm`] and [`crate::profile`] work over
//! Bluetooth without a single change.
//!
//! Discovered by enumerating the real device (see `examples/ble_scan.rs`):
//!
//! ```text
//! service 0xFFF0
//!   char 0xFFF1  NOTIFY | INDICATE              adapter -> us
//!   char 0xFFF2  WRITE | WRITE_WITHOUT_RESPONSE us -> adapter
//! ```
//!
//! Note this differs from the `main` branch of wican-fw, which declares
//! 0xFEE0/0xFEE1/0xFEE3 — that is the non-Pro firmware.

use crate::error::{Error, Result};
use crate::transport::Transport;
use btleplug::api::{
    bleuuid::uuid_from_u16, Central, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;
use tokio::runtime::Runtime;

/// Advertised name prefix of a WiCAN Pro: `WiC_<ble-mac>`.
pub const NAME_PREFIX: &str = "wic";

const SERVICE: u16 = 0xFFF0;
const CHAR_NOTIFY: u16 = 0xFFF1;
const CHAR_WRITE: u16 = 0xFFF2;

/// Conservative ATT payload size. ELM327 commands are far shorter, but
/// chunking keeps us safe if the negotiated MTU is the 23-byte minimum.
const CHUNK: usize = 20;

pub struct BleTransport {
    rt: Runtime,
    peripheral: Peripheral,
    write_char: Characteristic,
    rx: Receiver<Vec<u8>>,
    pending: VecDeque<u8>,
    timeout: Duration,
}

fn ble_err(e: impl std::fmt::Display) -> Error {
    Error::Adapter(format!("ble: {e}"))
}

impl BleTransport {
    /// Scan for a dongle whose name starts with `name_prefix`, connect, and
    /// subscribe to its notify characteristic.
    ///
    /// On macOS this must run from an app bundle declaring
    /// `NSBluetoothAlwaysUsageDescription` or the process is killed outright —
    /// see `scripts/macos-ble-bundle.sh`.
    pub fn connect(name_prefix: &str, scan_for: Duration, timeout: Duration) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(ble_err)?;

        let (peripheral, notify_char, write_char) = rt.block_on(async {
            let manager = Manager::new().await.map_err(ble_err)?;
            let adapter = manager
                .adapters()
                .await
                .map_err(ble_err)?
                .into_iter()
                .next()
                .ok_or_else(|| Error::Adapter("no Bluetooth adapter".into()))?;

            adapter
                .start_scan(ScanFilter::default())
                .await
                .map_err(ble_err)?;
            tokio::time::sleep(scan_for).await;
            adapter.stop_scan().await.map_err(ble_err)?;

            // Several OBD dongles advertise 0xFFF0, so match on the name.
            let wanted = name_prefix.to_lowercase();
            let mut found = None;
            for p in adapter.peripherals().await.map_err(ble_err)? {
                let Some(props) = p.properties().await.map_err(ble_err)? else {
                    continue;
                };
                let Some(name) = props.local_name else { continue };
                if name.to_lowercase().starts_with(&wanted) {
                    found = Some(p);
                    break;
                }
            }
            let peripheral = found.ok_or_else(|| {
                Error::Adapter(format!(
                    "no BLE device named {name_prefix}* found — the dongle stops \
                     advertising when the car is off"
                ))
            })?;

            peripheral.connect().await.map_err(ble_err)?;
            peripheral.discover_services().await.map_err(ble_err)?;

            let service_uuid = uuid_from_u16(SERVICE);
            let notify_uuid = uuid_from_u16(CHAR_NOTIFY);
            let write_uuid = uuid_from_u16(CHAR_WRITE);

            let chars: Vec<Characteristic> = peripheral
                .characteristics()
                .into_iter()
                .filter(|c| c.service_uuid == service_uuid)
                .collect();

            let notify_char = chars
                .iter()
                .find(|c| c.uuid == notify_uuid)
                .cloned()
                .ok_or_else(|| Error::Adapter("notify characteristic 0xFFF1 missing".into()))?;
            let write_char = chars
                .iter()
                .find(|c| c.uuid == write_uuid)
                .cloned()
                .ok_or_else(|| Error::Adapter("write characteristic 0xFFF2 missing".into()))?;

            peripheral
                .subscribe(&notify_char)
                .await
                .map_err(ble_err)?;

            Ok::<_, Error>((peripheral, notify_char, write_char))
        })?;

        // Pump notifications into a sync channel so `read` can block on it.
        let (tx, rx) = mpsc::channel();
        let notif_peripheral = peripheral.clone();
        let mut stream = rt
            .block_on(async { notif_peripheral.notifications().await })
            .map_err(ble_err)?;
        let notify_uuid = notify_char.uuid;
        rt.spawn(async move {
            while let Some(n) = stream.next().await {
                if n.uuid == notify_uuid && tx.send(n.value).is_err() {
                    break; // transport dropped
                }
            }
        });

        Ok(Self {
            rt,
            peripheral,
            write_char,
            rx,
            pending: VecDeque::new(),
            timeout,
        })
    }
}

impl Transport for BleTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let peripheral = self.peripheral.clone();
        let ch = self.write_char.clone();
        self.rt.block_on(async {
            for chunk in data.chunks(CHUNK) {
                peripheral
                    .write(&ch, chunk, WriteType::WithoutResponse)
                    .await
                    .map_err(ble_err)?;
            }
            Ok::<_, Error>(())
        })
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.pending.is_empty() {
            match self.rx.recv_timeout(self.timeout) {
                Ok(chunk) => self.pending.extend(chunk),
                Err(RecvTimeoutError::Timeout) => return Err(Error::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Adapter("ble notification stream ended".into()))
                }
            }
        }
        let n = buf.len().min(self.pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.pending.pop_front().expect("checked non-empty");
        }
        Ok(n)
    }
}

impl Drop for BleTransport {
    fn drop(&mut self) {
        let peripheral = self.peripheral.clone();
        // Best effort — the OS tears the link down anyway when we exit.
        let _ = self.rt.block_on(async { peripheral.disconnect().await });
    }
}
