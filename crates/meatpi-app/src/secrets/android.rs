//! Android Keystore backed storage for the login.
//!
//! The Keystore is only reachable from Java, so this drives it over JNI:
//!
//! 1. Get (or on first use, generate) an AES-256 key under `ALIAS` in the
//!    `AndroidKeyStore` provider. The key material stays in the secure
//!    hardware; the app only holds a handle.
//! 2. Encrypt with `AES/GCM/NoPadding`. The Keystore insists on choosing the
//!    IV itself, so it is read back from the cipher and stored alongside.
//! 3. Write `[iv_len][iv][ciphertext]` to the app's private data directory.
//!
//! Uninstalling the app deletes both the key and the file.

use jni::objects::{JByteArray, JObject, JValue};
use jni::{JNIEnv, JavaVM};
use std::path::PathBuf;
use std::sync::OnceLock;

const ALIAS: &str = "meatpi-meathandler-login";
const FILE: &str = "meathandler.login";
const ENCRYPT_MODE: i32 = 1;
const DECRYPT_MODE: i32 = 2;
const PURPOSE_ENCRYPT_DECRYPT: i32 = 1 | 2;
const GCM_TAG_BITS: i32 = 128;

static VM: OnceLock<JavaVM> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Call once from `android_main`, before anything touches the login.
pub fn init(app: &slint::android::AndroidApp) {
    // SAFETY: the pointer is the process-wide JavaVM, valid for the life of the app.
    if let Ok(vm) = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) } {
        let _ = VM.set(vm);
    }
    if let Some(dir) = app.internal_data_path() {
        let _ = DATA_DIR.set(dir);
    }
}

fn path() -> Result<PathBuf, String> {
    DATA_DIR
        .get()
        .map(|d| d.join(FILE))
        .ok_or_else(|| "secrets::init was not called".into())
}

/// Run `f` with a JNI env on this thread, turning a pending Java exception into
/// an error (and clearing it, or the next JNI call would abort the app).
fn with_env<T>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<T>) -> Result<T, String> {
    let vm = VM.get().ok_or("secrets::init was not called")?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let result = f(&mut env);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.map_err(|e| format!("keystore: {e}"))
}

fn string_array<'l>(env: &mut JNIEnv<'l>, value: &str) -> jni::errors::Result<JObject<'l>> {
    let s = env.new_string(value)?;
    Ok(env.new_object_array(1, "java/lang/String", &s)?.into())
}

/// The Keystore key handle, generating the key on first use.
fn key<'l>(env: &mut JNIEnv<'l>) -> jni::errors::Result<JObject<'l>> {
    let provider = env.new_string("AndroidKeyStore")?;
    let alias = env.new_string(ALIAS)?;

    let ks = env
        .call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[JValue::Object(&provider)],
        )?
        .l()?;
    env.call_method(
        &ks,
        "load",
        "(Ljava/security/KeyStore$LoadStoreParameter;)V",
        &[JValue::Object(&JObject::null())],
    )?;

    let exists = env
        .call_method(&ks, "containsAlias", "(Ljava/lang/String;)Z", &[JValue::Object(&alias)])?
        .z()?;
    if !exists {
        let aes = env.new_string("AES")?;
        let generator = env
            .call_static_method(
                "javax/crypto/KeyGenerator",
                "getInstance",
                "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
                &[JValue::Object(&aes), JValue::Object(&provider)],
            )?
            .l()?;
        let builder = env.new_object(
            "android/security/keystore/KeyGenParameterSpec$Builder",
            "(Ljava/lang/String;I)V",
            &[JValue::Object(&alias), JValue::Int(PURPOSE_ENCRYPT_DECRYPT)],
        )?;
        const BUILDER: &str = "Landroid/security/keystore/KeyGenParameterSpec$Builder;";
        let gcm = string_array(env, "GCM")?;
        env.call_method(
            &builder,
            "setBlockModes",
            format!("([Ljava/lang/String;){BUILDER}"),
            &[JValue::Object(&gcm)],
        )?;
        let no_padding = string_array(env, "NoPadding")?;
        env.call_method(
            &builder,
            "setEncryptionPaddings",
            format!("([Ljava/lang/String;){BUILDER}"),
            &[JValue::Object(&no_padding)],
        )?;
        env.call_method(&builder, "setKeySize", format!("(I){BUILDER}"), &[JValue::Int(256)])?;
        let spec = env
            .call_method(&builder, "build", "()Landroid/security/keystore/KeyGenParameterSpec;", &[])?
            .l()?;
        env.call_method(
            &generator,
            "init",
            "(Ljava/security/spec/AlgorithmParameterSpec;)V",
            &[JValue::Object(&spec)],
        )?;
        env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[])?;
    }

    env.call_method(
        &ks,
        "getKey",
        "(Ljava/lang/String;[C)Ljava/security/Key;",
        &[JValue::Object(&alias), JValue::Object(&JObject::null())],
    )?
    .l()
}

fn cipher<'l>(env: &mut JNIEnv<'l>) -> jni::errors::Result<JObject<'l>> {
    let transform = env.new_string("AES/GCM/NoPadding")?;
    env.call_static_method(
        "javax/crypto/Cipher",
        "getInstance",
        "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
        &[JValue::Object(&transform)],
    )?
    .l()
}

fn do_final(env: &mut JNIEnv, cipher: &JObject, input: &[u8]) -> jni::errors::Result<Vec<u8>> {
    let input = env.byte_array_from_slice(input)?;
    let out = env.call_method(cipher, "doFinal", "([B)[B", &[JValue::Object(&input)])?.l()?;
    env.convert_byte_array(JByteArray::from(out))
}

fn encrypt(plain: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    with_env(|env| {
        let key = key(env)?;
        let cipher = cipher(env)?;
        env.call_method(
            &cipher,
            "init",
            "(ILjava/security/Key;)V",
            &[JValue::Int(ENCRYPT_MODE), JValue::Object(&key)],
        )?;
        let iv = env.call_method(&cipher, "getIV", "()[B", &[])?.l()?;
        let iv = env.convert_byte_array(JByteArray::from(iv))?;
        let ct = do_final(env, &cipher, plain)?;
        Ok((iv, ct))
    })
}

fn decrypt(iv: &[u8], ct: &[u8]) -> Result<Vec<u8>, String> {
    with_env(|env| {
        let key = key(env)?;
        let cipher = cipher(env)?;
        let iv = env.byte_array_from_slice(iv)?;
        let spec = env.new_object(
            "javax/crypto/spec/GCMParameterSpec",
            "(I[B)V",
            &[JValue::Int(GCM_TAG_BITS), JValue::Object(&iv)],
        )?;
        env.call_method(
            &cipher,
            "init",
            "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
            &[JValue::Int(DECRYPT_MODE), JValue::Object(&key), JValue::Object(&spec)],
        )?;
        do_final(env, &cipher, ct)
    })
}

pub fn save(blob: &str) -> Result<(), String> {
    let (iv, ct) = encrypt(blob.as_bytes())?;
    let iv_len = u8::try_from(iv.len()).map_err(|_| "IV too long")?;
    let mut file = Vec::with_capacity(1 + iv.len() + ct.len());
    file.push(iv_len);
    file.extend_from_slice(&iv);
    file.extend_from_slice(&ct);
    let path = path()?;
    // Write then rename, so a crash mid-write never leaves half a file.
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &file).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn load() -> Result<Option<String>, String> {
    let data = match std::fs::read(path()?) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let (&iv_len, rest) = data.split_first().ok_or("empty login file")?;
    let iv_len = iv_len as usize;
    if rest.len() <= iv_len {
        return Err("truncated login file".into());
    }
    let plain = decrypt(&rest[..iv_len], &rest[iv_len..])?;
    String::from_utf8(plain).map(Some).map_err(|e| e.to_string())
}

pub fn forget() -> Result<(), String> {
    match std::fs::remove_file(path()?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
