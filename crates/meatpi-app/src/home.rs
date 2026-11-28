//! The Home button: opens Home Assistant, which MeatHandler forwards on port
//! 8444 of the same host (see meathandler-server/src/ha_proxy.rs).
//!
//! - Android: launches the Home Assistant companion app, which keeps its own
//!   login. If it is not installed, the UI offers the Play Store or the
//!   browser instead.
//! - Desktop: opens the page in the default browser.

/// Package id of the official Home Assistant companion app (Play Store).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const HA_PACKAGE: &str = "io.homeassistant.companion.android";
/// Every build of it worth launching: the Play Store one, then the "minimal"
/// one from F-Droid/GitHub. Both must also be in Cargo.toml's
/// `[package.metadata.android.queries]`, or Android hides them from us.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub const HA_PACKAGES: &[&str] = &[HA_PACKAGE, "io.homeassistant.companion.android.minimal"];
const STORE_WEB: &str = "https://play.google.com/store/apps/details?id=io.homeassistant.companion.android";

/// Home Assistant's address: the MeatHandler host on port 8444.
pub fn ha_url(meathandler_url: &str) -> String {
    let rest = meathandler_url.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let host = authority.split(':').next().unwrap_or("");
    let host = if host.is_empty() { "meathandler.ddns.net" } else { host };
    format!("https://{host}:8444/")
}

pub enum Launch {
    Opened,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    NotInstalled,
}

/// Open the companion app on Android. Elsewhere there is no app, so this
/// opens `url` in the browser.
pub fn open_app(url: &str) -> Result<Launch, String> {
    #[cfg(target_os = "android")]
    {
        let _ = url;
        android::launch_first(HA_PACKAGES)
            .map(|found| if found { Launch::Opened } else { Launch::NotInstalled })
    }
    #[cfg(not(target_os = "android"))]
    {
        open_url(url).map(|_| Launch::Opened)
    }
}

/// The Play Store page for the companion app.
pub fn open_store() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let market = format!("market://details?id={HA_PACKAGE}");
        if android::view(&market)? {
            return Ok(());
        }
    }
    open_url(STORE_WEB)
}

pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        return match android::view(url)? {
            true => Ok(()),
            false => Err("no app can open web links".into()),
        };
    }
    #[cfg(not(target_os = "android"))]
    {
        use std::process::Command;
        #[cfg(target_os = "macos")]
        let mut cmd = Command::new("open");
        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let mut cmd = Command::new("xdg-open");
        cmd.arg(url).spawn().map(|_| ()).map_err(|e| format!("could not open {url}: {e}"))
    }
}

#[cfg(target_os = "android")]
pub use android::init;

#[cfg(target_os = "android")]
mod android {
    use jni::objects::{JObject, JValue};
    use jni::{JNIEnv, JavaVM};
    use slint::android::AndroidApp;
    use std::sync::OnceLock;

    const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
    static APP: OnceLock<AndroidApp> = OnceLock::new();

    /// Call once from `android_main`.
    pub fn init(app: AndroidApp) {
        let _ = APP.set(app);
    }

    /// Run `f` with a JNI env and the current activity, clearing any pending
    /// Java exception afterwards so the next JNI call does not abort the app.
    fn with_activity<T>(
        f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>,
    ) -> Result<T, String> {
        let app = APP.get().ok_or("home::init was not called")?;
        // SAFETY: both pointers come from the running activity and stay valid
        // while it is alive, which it is while its UI thread handles a click.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }
            .map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
        let result = f(&mut env, &activity);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        result.map_err(|e| format!("android: {e}"))
    }

    /// `startActivity`, returning false when no installed app can handle the
    /// intent. Catching ActivityNotFoundException is Android's recommended
    /// check: it needs no package-visibility (<queries>) entry in the manifest.
    fn start(env: &mut JNIEnv, activity: &JObject, intent: &JObject) -> jni::errors::Result<bool> {
        env.call_method(intent, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(FLAG_ACTIVITY_NEW_TASK)])?;
        match env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(intent)]) {
            Ok(_) => Ok(true),
            Err(jni::errors::Error::JavaException) => {
                let ex = env.exception_occurred()?;
                env.exception_clear()?;
                if env.is_instance_of(&ex, "android/content/ActivityNotFoundException")? {
                    Ok(false)
                } else {
                    Err(jni::errors::Error::JavaException)
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Launch the first of `packages` that is installed. False if none are.
    /// Uses PackageManager's own launch intent for each app, which only finds
    /// apps listed under `queries` in the manifest.
    pub fn launch_first(packages: &[&str]) -> Result<bool, String> {
        with_activity(|env, activity| {
            let pm = env
                .call_method(activity, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?
                .l()?;
            for package in packages {
                let name = env.new_string(package)?;
                let intent = env
                    .call_method(
                        &pm,
                        "getLaunchIntentForPackage",
                        "(Ljava/lang/String;)Landroid/content/Intent;",
                        &[JValue::Object(&name)],
                    )?
                    .l()?;
                if intent.is_null() {
                    continue;
                }
                if start(env, activity, &intent)? {
                    log::info!("opened {package}");
                    return Ok(true);
                }
            }
            log::info!("no Home Assistant app found ({})", packages.join(", "));
            Ok(false)
        })
    }

    /// ACTION_VIEW on a URL. False if nothing can open it.
    pub fn view(url: &str) -> Result<bool, String> {
        with_activity(|env, activity| {
            let text = env.new_string(url)?;
            let uri = env
                .call_static_method("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[JValue::Object(&text)])?
                .l()?;
            let action = env.new_string("android.intent.action.VIEW")?;
            let intent = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[JValue::Object(&action), JValue::Object(&uri)],
            )?;
            start(env, activity, &intent)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ha_url;

    #[test]
    fn same_host_port_8444() {
        assert_eq!(ha_url("https://meathandler.ddns.net:8443"), "https://meathandler.ddns.net:8444/");
        assert_eq!(ha_url("https://meathandler.ddns.net:8443/api/latest"), "https://meathandler.ddns.net:8444/");
        assert_eq!(ha_url("http://10.43.1.57:8080"), "https://10.43.1.57:8444/");
        assert_eq!(ha_url("meathandler.ddns.net"), "https://meathandler.ddns.net:8444/");
        assert_eq!(ha_url("https://user:pw@meathandler.ddns.net:8443"), "https://meathandler.ddns.net:8444/");
        assert_eq!(ha_url(""), "https://meathandler.ddns.net:8444/");
    }
}
