use std::{
    process::{Command, Stdio},
    str::FromStr,
    thread,
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use liana::miniscript::bitcoin::{bip32::DerivationPath, Network};

pub mod serde;
pub mod subscription;

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

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

/// Takes an exclusive advisory lock on `file`, off the async runtime as
/// acquisition blocks while another process holds the lock. The lock is
/// released when the returned file is dropped.
pub async fn lock_write(file: tokio::fs::File) -> std::io::Result<tokio::fs::File> {
    let std_file = file.into_std().await;
    let std_file = tokio::task::spawn_blocking(move || {
        fs2::FileExt::lock_exclusive(&std_file).map(|()| std_file)
    })
    .await
    .expect("locking task does not panic")?;
    Ok(tokio::fs::File::from_std(std_file))
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
