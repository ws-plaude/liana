use std::{
    future::Future,
    process::{Command, Stdio},
    str::FromStr,
    thread,
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use futures::channel::oneshot::{self, Canceled};
use liana::miniscript::bitcoin::{bip32::DerivationPath, Network};

pub mod serde;
pub mod subscription;

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

/// Encodes `pairs` as an application/x-www-form-urlencoded query string.
///
/// Same byte set as the WHATWG serializer: space becomes `+`, the unreserved characters and
/// `*-._` are kept, everything else is percent-encoded.
pub fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    fn encode(value: &str, out: &mut String) {
        for byte in value.as_bytes() {
            match byte {
                b' ' => out.push('+'),
                b'*' | b'-' | b'.' | b'_' => out.push(*byte as char),
                b if b.is_ascii_alphanumeric() => out.push(*b as char),
                b => out.push_str(&format!("%{b:02X}")),
            }
        }
    }

    let mut query = String::new();
    for (key, value) in pairs {
        if !query.is_empty() {
            query.push('&');
        }
        encode(key, &mut query);
        query.push('=');
        encode(value, &mut query);
    }
    query
}

/// Opens `url` with the platform default handler, without blocking.
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    let mut cmd = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "start", ""]);
        cmd
    };
    let mut child = cmd
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // reap in the background so the handler process does not linger as a zombie
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Practical email check for form hinting: one '@', a non-empty local part
/// and a dotted domain (TLD required). The backend stays authoritative.
pub fn is_valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || email.contains(char::is_whitespace) {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Runs `f` on its own thread so async code can call a blocking API without holding an executor
/// thread. The future resolves once `f` returns, or errors if the thread died without answering.
///
/// A thread cannot be cancelled: dropping the returned future does not stop `f`.
pub fn spawn_blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Result<T, Canceled>> {
    let (tx, rx) = oneshot::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

/// Takes an exclusive advisory lock on `file`, off the executor as acquisition
/// blocks while another process holds the lock. The lock is released when the
/// returned file is dropped.
pub async fn lock_write(file: std::fs::File) -> std::io::Result<std::fs::File> {
    spawn_blocking(move || fs2::FileExt::lock_exclusive(&file).map(|()| file))
        .await
        .map_err(|e| std::io::Error::other(format!("locking thread died: {e}")))?
}

/// Returns the current time as a [`Duration`] since the UNIX epoch.
pub fn now() -> Duration {
    now_fallible().expect("cannot fail")
}

/// Faliible version of [`now`].
pub fn now_fallible() -> Result<Duration, SystemTimeError> {
    SystemTime::now().duration_since(UNIX_EPOCH)
}

pub fn default_derivation_path(network: Network) -> DerivationPath {
    // Note that "m" is ignored when parsing string and could be removed:
    // https://github.com/rust-bitcoin/rust-bitcoin/pull/2677
    DerivationPath::from_str({
        if network == Network::Bitcoin {
            "m/48'/0'/0'/2'"
        } else {
            "m/48'/1'/0'/2'"
        }
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::{form_urlencode, is_valid_email};

    #[test]
    fn form_urlencoding() {
        assert_eq!(form_urlencode(&[("a", "b")]), "a=b");
        assert_eq!(form_urlencode(&[("a", "b"), ("c", "d")]), "a=b&c=d");
        assert_eq!(
            form_urlencode(&[("subject", "hi there")]),
            "subject=hi+there"
        );
        // Newlines, quotes and the separators themselves must not survive unescaped.
        assert_eq!(
            form_urlencode(&[("body", "a\nb\"c\"&d=e")]),
            "body=a%0Ab%22c%22%26d%3De"
        );
        assert_eq!(form_urlencode(&[("k", "*-._")]), "k=*-._");
        assert_eq!(form_urlencode(&[("k", "é")]), "k=%C3%A9");
        assert_eq!(form_urlencode(&[]), "");
    }

    #[test]
    fn email_validation() {
        assert!(is_valid_email("a@b.co"));
        assert!(is_valid_email("first.last@sub.domain.org"));
        assert!(is_valid_email("user+tag@wizardsardine.com"));
        assert!(!is_valid_email(""));
        assert!(!is_valid_email("no-at-sign.com"));
        assert!(!is_valid_email("a@no-tld"));
        assert!(!is_valid_email("a@domain."));
        assert!(!is_valid_email("@domain.com"));
        assert!(!is_valid_email("with space@domain.com"));
        assert!(!is_valid_email("a@-bad.com"));
    }
}
