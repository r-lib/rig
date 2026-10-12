//! The system credential store: macOS Keychain, Windows Credential Manager,
//! or a Secret Service (GNOME Keyring, KWallet) on Linux.
//!
//! Entries are keyed the same way as in R's keyring package, so a password
//! stored with `keyring::key_set()` (and used by pak) is found by rig, and
//! the other way around.

#[cfg(target_os = "macos")]
pub const BACKEND: &str = "macos";
#[cfg(target_os = "windows")]
pub const BACKEND: &str = "wincred";
#[cfg(target_os = "linux")]
pub const BACKEND: &str = "secret service";

/// R's `utils::URLencode(reserved = FALSE)`, used by keyring's Windows
/// backend for the parts of the target name.
#[cfg(any(target_os = "windows", test))]
fn r_urlencode(x: &str) -> String {
    let mut out = String::new();
    for b in x.bytes() {
        let ok = b.is_ascii_alphanumeric() || b"[]!$&'()*+,;=:/?@#._~-".contains(&b);
        if ok {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn entry(service: &str, user: &str) -> keyring::Result<keyring::Entry> {
    #[cfg(target_os = "windows")]
    {
        // keyring (R): "<keyring>:<service>:<username>", for the default
        // keyring the first part is empty.
        let target = format!(":{}:{}", r_urlencode(service), r_urlencode(user));
        keyring::Entry::new_with_target(&target, service, user)
    }
    #[cfg(not(target_os = "windows"))]
    keyring::Entry::new(service, user)
}

/// Run `f` on a fresh thread. The keyring backends may block on their own
/// event loop, which must not happen inside the tokio runtime of a download.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| s.spawn(f).join().expect("keyring thread panicked"))
}

/// The password of `user` for `service`. `Ok(None)` if there is no such
/// entry, `Err` if the credential store itself failed, e.g. there is no
/// Secret Service running.
pub fn get(service: &str, user: &str) -> Result<Option<String>, String> {
    off_runtime(
        || match entry(service, user).and_then(|e| e.get_password()) {
            Ok(pw) => Ok(Some(pw)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        },
    )
}

pub fn set(service: &str, user: &str, password: &str) -> Result<(), String> {
    off_runtime(|| {
        entry(service, user)
            .and_then(|e| e.set_password(password))
            .map_err(|e| e.to_string())
    })
}

/// `Ok(false)` if there was no such entry.
pub fn delete(service: &str, user: &str) -> Result<bool, String> {
    off_runtime(
        || match entry(service, user).and_then(|e| e.delete_credential()) {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(e.to_string()),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_r_urlencode() {
        assert_eq!(
            r_urlencode("https://user@host.com/a b"),
            "https://user@host.com/a%20b"
        );
    }
}
