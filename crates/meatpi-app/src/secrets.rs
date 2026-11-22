//! Remembering the MeatHandler login in the operating system's secret store.
//!
//! - macOS: Keychain. Windows: Credential Manager. Linux: Secret Service
//!   (GNOME Keyring / KWallet). All through the `keyring` crate.
//! - Android: an AES-256-GCM key generated inside the Android Keystore (it
//!   never leaves the secure hardware) encrypts the login, and only the
//!   ciphertext is written to the app's private storage. See `android.rs`.
//!
//! The URL, user and password are stored together as one secret, so there is
//! nothing to look up first and nothing sensitive on disk in the clear.

#[cfg(target_os = "android")]
mod android;

#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub url: String,
    pub user: String,
    pub pass: String,
}

impl Login {
    fn encode(&self) -> String {
        format!("v1\n{}\n{}\n{}", self.url, self.user, self.pass)
    }

    fn decode(blob: &str) -> Option<Self> {
        // splitn keeps any newline that might be in the password itself.
        let mut parts = blob.splitn(4, '\n');
        if parts.next()? != "v1" {
            return None;
        }
        let login = Login {
            url: parts.next()?.to_string(),
            user: parts.next()?.to_string(),
            pass: parts.next()?.to_string(),
        };
        (!login.pass.is_empty()).then_some(login)
    }
}

/// The saved login, if there is one and it can be read.
pub fn load() -> Option<Login> {
    match backend::load() {
        Ok(Some(blob)) => Login::decode(&blob),
        Ok(None) => None,
        Err(e) => {
            log::warn!("could not read saved login: {e}");
            None
        }
    }
}

pub fn save(login: &Login) {
    match backend::save(&login.encode()) {
        Ok(()) => log::info!("saved MeatHandler login"),
        Err(e) => log::warn!("could not save login: {e}"),
    }
}

pub fn forget() {
    if let Err(e) = backend::forget() {
        log::warn!("could not forget login: {e}");
    }
}

#[cfg(target_os = "android")]
pub use android::init;

#[cfg(target_os = "android")]
mod backend {
    pub use super::android::{forget, load, save};
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[cfg(not(target_os = "android"))]
mod backend {
    const SERVICE: &str = "meatpi-dashboard";
    const ACCOUNT: &str = "meathandler";

    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
    }

    pub fn load() -> Result<Option<String>, String> {
        match entry()?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn save(blob: &str) -> Result<(), String> {
        entry()?.set_password(blob).map_err(|e| e.to_string())
    }

    pub fn forget() -> Result<(), String> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Anywhere else: remember nothing rather than store a password badly.
#[cfg(not(any(
    target_os = "android",
    target_os = "macos",
    target_os = "windows",
    target_os = "linux"
)))]
mod backend {
    pub fn load() -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn save(_: &str) -> Result<(), String> {
        Err("no secret store on this platform".into())
    }
    pub fn forget() -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Login;

    #[test]
    fn round_trips_including_awkward_passwords() {
        let login = Login {
            url: "https://meathandler.ddns.net:8443".into(),
            user: "enoch".into(),
            pass: "p@ss\nword:with:colons".into(),
        };
        assert_eq!(Login::decode(&login.encode()), Some(login));
    }

    #[test]
    fn ignores_unknown_or_empty_blobs() {
        assert_eq!(Login::decode("v2\na\nb\nc"), None);
        assert_eq!(Login::decode("v1\nurl\nuser\n"), None);
        assert_eq!(Login::decode(""), None);
    }
}
