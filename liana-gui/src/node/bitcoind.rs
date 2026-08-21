use base64::Engine;
use liana::{
    miniscript::bitcoin::{
        self,
        hashes::{sha256, Hash, HashEngine, Hmac, HmacEngine},
        Network,
    },
    random::{random_bytes, RandomnessError},
};
use liana_ui::component::form;
use lianad::config::BitcoindConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time;

use tracing::{info, warn};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use crate::dir::{BitcoindDirectory, LianaDirectory};
use crate::utils::now_fallible;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(target_os = "windows")]
const DETACHED_PROCESS: u32 = 0x00000008;

/// Current and previous managed bitcoind versions, in order of descending version.
pub const VERSIONS: [&str; 7] = ["29.0", "28.0", "27.1", "26.1", "26.0", "25.1", "25.0"];

/// Current managed bitcoind version for new installations.
pub const VERSION: &str = VERSIONS[0];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "5bb824fc86a15318d6a83a1b821ff4cd4b3d3d0e1ec3d162b805ccf7cae6fca8";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub const SHA256SUM: &str = "34431c582a0399dd42e1276d87d25306cbdde0217f6744bd55a2945986645dda";

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "a681e4f6ce524c338a105f214613605bac6c33d58c31dc5135bbc02bc458bb6c";

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub const SHA256SUM: &str = "4c1780532031129fcacfc0e393c8430b3cea414c9f8c5e0c0c87ebe59a5ada1b";

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub fn download_filename() -> String {
    format!("bitcoin-{}-x86_64-apple-darwin.tar.gz", &VERSION)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn download_filename() -> String {
    format!("bitcoin-{}-arm64-apple-darwin.tar.gz", &VERSION)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn download_filename() -> String {
    format!("bitcoin-{}-x86_64-linux-gnu.tar.gz", &VERSION)
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn download_filename() -> String {
    format!("bitcoin-{}-win64.zip", &VERSION)
}

pub fn download_url() -> String {
    format!(
        "https://bitcoincore.org/bin/bitcoin-core-{}/{}",
        &VERSION,
        download_filename()
    )
}

pub fn internal_bitcoind_directory(liana_datadir: &LianaDirectory) -> PathBuf {
    liana_datadir.bitcoind_directory().path().to_path_buf()
}

/// Data directory used by internal bitcoind.
pub fn internal_bitcoind_datadir(liana_datadir: &LianaDirectory) -> PathBuf {
    let mut datadir = internal_bitcoind_directory(liana_datadir);
    datadir.push("datadir");
    datadir
}

/// Internal bitcoind executable path.
pub fn internal_bitcoind_exe_path(
    liana_datadir: &LianaDirectory,
    bitcoind_version: &str,
) -> PathBuf {
    internal_bitcoind_directory(liana_datadir)
        .join(format!("bitcoin-{bitcoind_version}"))
        .join("bin")
        .join(if cfg!(target_os = "windows") {
            "bitcoind.exe"
        } else {
            "bitcoind"
        })
}

/// Path of the `bitcoin.conf` file used by internal bitcoind.
pub fn internal_bitcoind_config_path(bitcoind_datadir: &Path) -> PathBuf {
    let mut config_path = PathBuf::from(bitcoind_datadir);
    config_path.push("bitcoin.conf");
    config_path
}

/// Path of the cookie file used by internal bitcoind on a given network.
pub fn internal_bitcoind_cookie_path(bitcoind_datadir: &Path, network: &Network) -> PathBuf {
    let mut cookie_path = bitcoind_datadir.to_path_buf();
    if let Some(dir) = bitcoind_network_dir(network) {
        cookie_path.push(dir);
    }
    cookie_path.push(".cookie");
    cookie_path
}

/// Path of the cookie file used by internal bitcoind on a given network.
pub fn internal_bitcoind_debug_log_path(
    lianad_datadir: &LianaDirectory,
    network: Network,
) -> PathBuf {
    let mut debug_log_path = internal_bitcoind_datadir(lianad_datadir);
    if let Some(dir) = bitcoind_network_dir(&network) {
        debug_log_path.push(dir);
    }
    debug_log_path.push("debug.log");
    debug_log_path
}

pub fn bitcoind_network_dir(network: &Network) -> Option<String> {
    let dir = match network {
        Network::Bitcoin => {
            return None;
        }
        Network::Testnet => "testnet3",
        Network::Testnet4 => "testnet4",
        Network::Regtest => "regtest",
        Network::Signet => "signet",
    };
    Some(dir.to_string())
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum RpcAuthParseError {
    MissingColon,
    MissingDollarSign,
}

impl std::fmt::Display for RpcAuthParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::MissingColon => write!(
                f,
                "RPC auth string should contain colon between user and salt."
            ),
            Self::MissingDollarSign => write!(
                f,
                "RPC auth string should contain dollar sign between salt and password HMAC."
            ),
        }
    }
}

/// Represents RPC auth credentials as stored in bitcoin.conf.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct RpcAuth {
    pub user: String,
    salt: String,
    password_hmac: String,
}

impl RpcAuth {
    /// Returns a new `RpcAuth` object for the given `user` with a random salt and password.
    /// This random password is also returned.
    pub fn new(user: &str) -> Result<(Self, String), RandomnessError> {
        // RPC auth generation follows approach in
        // https://github.com/bitcoin/bitcoin/blob/master/share/rpcauth/rpcauth.py
        let password =
            random_bytes().map(|bytes| base64::prelude::BASE64_URL_SAFE_NO_PAD.encode(bytes))?;
        // As per the Python script, only use 16 bytes for the salt.
        let salt = random_bytes().map(|bytes| hex::encode(&bytes[..16]))?;
        let mut engine = HmacEngine::<sha256::Hash>::new(salt.as_bytes());
        engine.input(password.as_bytes());
        let password_hmac = Hmac::<sha256::Hash>::from_engine(engine);

        Ok((
            Self {
                user: user.to_string(),
                salt,
                password_hmac: hex::encode(&password_hmac[..]),
            },
            password,
        ))
    }
}

impl std::fmt::Display for RpcAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}${}", self.user, self.salt, self.password_hmac)
    }
}

impl std::str::FromStr for RpcAuth {
    type Err = RpcAuthParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (user, salt_pw) = s.split_once(':').ok_or(RpcAuthParseError::MissingColon)?;
        let (salt, pw) = salt_pw
            .split_once('$')
            .ok_or(RpcAuthParseError::MissingDollarSign)?;
        Ok(Self {
            user: user.to_string(),
            salt: salt.to_string(),
            password_hmac: pw.to_string(),
        })
    }
}

/// Represents section for a single network in `bitcoin.conf` file.
#[derive(PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct InternalBitcoindNetworkConfig {
    pub rpc_port: u16,
    pub p2p_port: u16,
    pub prune: u32,
    pub rpc_auth: Option<RpcAuth>,
}

/// Represents the `bitcoin.conf` file to be used by internal bitcoind.
#[derive(Debug, Clone)]
pub struct InternalBitcoindConfig {
    pub networks: BTreeMap<Network, InternalBitcoindNetworkConfig>,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum InternalBitcoindConfigError {
    KeyNotFound(String),
    CouldNotParseValue(String),
    UnexpectedSection(String),
    TooManyElements(String),
    FileNotFound,
    ReadingFile(String),
    WritingFile(String),
    Unexpected(String),
}

impl std::fmt::Display for InternalBitcoindConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::KeyNotFound(e) => write!(f, "Config file does not contain expected key: {e}"),
            Self::CouldNotParseValue(e) => write!(f, "Value could not be parsed: {e}"),
            Self::UnexpectedSection(e) => write!(f, "Unexpected section in file: {e}"),
            Self::TooManyElements(section) => {
                write!(f, "Section in file contains too many elements: {section}")
            }
            Self::FileNotFound => write!(f, "File not found"),
            Self::ReadingFile(e) => write!(f, "Error while reading file: {e}"),
            Self::WritingFile(e) => write!(f, "Error while writing file: {e}"),
            Self::Unexpected(e) => write!(f, "Unexpected error: {e}"),
        }
    }
}

impl Default for InternalBitcoindConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl InternalBitcoindConfig {
    pub fn new() -> Self {
        Self {
            networks: BTreeMap::new(),
        }
    }

    pub fn from_file(path: &PathBuf) -> Result<Self, InternalBitcoindConfigError> {
        if !path.exists() {
            return Err(InternalBitcoindConfigError::FileNotFound);
        }
        std::fs::read_to_string(path)
            .map_err(|e| InternalBitcoindConfigError::ReadingFile(e.to_string()))?
            .parse()
    }

    pub fn to_file(&self, path: &PathBuf) -> Result<(), InternalBitcoindConfigError> {
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| InternalBitcoindConfigError::Unexpected("No parent".to_string()))?,
        )
        .map_err(|e| InternalBitcoindConfigError::Unexpected(e.to_string()))?;
        info!("Writing to file {}", path.to_string_lossy());
        std::fs::write(path, self.to_string())
            .map_err(|e| InternalBitcoindConfigError::WritingFile(e.to_string()))?;

        Ok(())
    }
}

fn network_config_from_props(
    section: &str,
    props: &BTreeMap<String, String>,
) -> Result<(Network, InternalBitcoindNetworkConfig), InternalBitcoindConfigError> {
    let network = Network::from_core_arg(section)
        .map_err(|e| InternalBitcoindConfigError::UnexpectedSection(e.to_string()))?;
    if props.len() > 4 {
        return Err(InternalBitcoindConfigError::TooManyElements(
            section.to_string(),
        ));
    }
    let rpc_port = props
        .get("rpcport")
        .ok_or_else(|| InternalBitcoindConfigError::KeyNotFound("rpcport".to_string()))?
        .parse::<u16>()
        .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))?;
    let p2p_port = props
        .get("port")
        .ok_or_else(|| InternalBitcoindConfigError::KeyNotFound("port".to_string()))?
        .parse::<u16>()
        .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))?;
    let prune = props
        .get("prune")
        .ok_or_else(|| InternalBitcoindConfigError::KeyNotFound("prune".to_string()))?
        .parse::<u32>()
        .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))?;
    let rpc_auth = props
        .get("rpcauth")
        .map(|v| {
            v.parse::<RpcAuth>()
                .map_err(|e| InternalBitcoindConfigError::CouldNotParseValue(e.to_string()))
        })
        .transpose()?;

    Ok((
        network,
        InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune,
            rpc_auth,
        },
    ))
}

impl std::str::FromStr for InternalBitcoindConfig {
    type Err = InternalBitcoindConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut networks = BTreeMap::new();
        let mut current: Option<(String, BTreeMap<String, String>)> = None;
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(section) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                if let Some((prev, props)) = current.take() {
                    let (network, conf) = network_config_from_props(&prev, &props)?;
                    networks.insert(network, conf);
                }
                current = Some((section.to_string(), BTreeMap::new()));
            } else {
                let (key, value) = line.split_once('=').ok_or_else(|| {
                    InternalBitcoindConfigError::CouldNotParseValue(line.to_string())
                })?;
                match current.as_mut() {
                    Some((_, props)) => {
                        props.insert(key.trim().to_string(), value.trim().to_string());
                    }
                    None => {
                        return Err(InternalBitcoindConfigError::UnexpectedSection(
                            "General section should be empty".to_string(),
                        ))
                    }
                }
            }
        }
        if let Some((prev, props)) = current {
            let (network, conf) = network_config_from_props(&prev, &props)?;
            networks.insert(network, conf);
        }
        Ok(Self { networks })
    }
}

impl fmt::Display for InternalBitcoindConfig {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (i, (network, conf)) in self.networks.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            writeln!(f, "[{}]", network.to_core_arg())?;
            writeln!(f, "rpcport={}", conf.rpc_port)?;
            writeln!(f, "port={}", conf.p2p_port)?;
            writeln!(f, "prune={}", conf.prune)?;
            if let Some(rpc_auth) = conf.rpc_auth.as_ref() {
                writeln!(f, "rpcauth={rpc_auth}")?;
            }
        }
        Ok(())
    }
}

/// Possible errors when starting bitcoind.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum StartInternalBitcoindError {
    Lock(String),
    CommandError(String),
    CouldNotCanonicalizeDataDir(String),
    BitcoinDError(String),
    ExecutableNotFound,
    ProcessExited(std::process::ExitStatus),
}

impl std::fmt::Display for StartInternalBitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Lock(e) => {
                write!(f, "lock file error: {e}")
            }
            Self::CommandError(e) => {
                write!(f, "Command to start bitcoind returned an error: {e}")
            }
            Self::CouldNotCanonicalizeDataDir(e) => {
                write!(f, "Failed to canonicalize datadir: {e}")
            }
            Self::BitcoinDError(e) => write!(f, "bitcoind connection check failed: {e}"),
            Self::ExecutableNotFound => write!(f, "bitcoind executable not found."),
            Self::ProcessExited(status) => {
                write!(f, "bitcoind process exited with status '{status}'.")
            }
        }
    }
}
#[derive(Debug, Clone)]
pub struct Bitcoind {
    pub config: BitcoindConfig,
    lock: LockFile,
}

impl Bitcoind {
    /// Start internal bitcoind for the given network.
    pub fn maybe_start(
        network: bitcoin::Network,
        config: BitcoindConfig,
        liana_datadir: &LianaDirectory,
    ) -> Result<Self, StartInternalBitcoindError> {
        if lianad::BitcoinD::new(&config, "internal_bitcoind_start".to_string()).is_ok() {
            info!("Internal bitcoind is already running");
            return Ok(Bitcoind {
                config,
                lock: LockFile::create(liana_datadir.bitcoind_directory(), network)
                    .map_err(|e| StartInternalBitcoindError::Lock(format!("{e:?}")))?,
            });
        }
        let bitcoind_datadir = internal_bitcoind_datadir(liana_datadir);
        // Find most recent bitcoind version available.
        let bitcoind_exe_path = VERSIONS
            .iter()
            .filter_map(|v| {
                let path = internal_bitcoind_exe_path(liana_datadir, v);
                if path.exists() {
                    Some(path)
                } else {
                    None
                }
            })
            .next()
            .ok_or(StartInternalBitcoindError::ExecutableNotFound)?;
        info!(
            "Found bitcoind executable at '{}'.",
            bitcoind_exe_path.to_string_lossy()
        );
        let datadir_path_str = bitcoind_datadir
            .canonicalize()
            .map_err(|e| StartInternalBitcoindError::CouldNotCanonicalizeDataDir(e.to_string()))?
            .to_str()
            .ok_or_else(|| {
                StartInternalBitcoindError::CouldNotCanonicalizeDataDir(
                    "Couldn't convert path to str.".to_string(),
                )
            })?
            .to_string();

        // See https://github.com/rust-lang/rust/issues/42869.
        #[cfg(target_os = "windows")]
        let datadir_path_str = datadir_path_str.replace("\\\\?\\", "").replace("\\\\?", "");

        let args = vec![
            format!("-chain={}", network.to_core_arg()),
            format!("-datadir={}", datadir_path_str),
        ];
        let mut command = std::process::Command::new(bitcoind_exe_path);

        #[cfg(target_os = "windows")]
        let command = command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Create a new session to detach the child from the main process.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }

        let mut process = command
            .args(&args)
            // FIXME: can we pipe stderr to our logging system somehow?
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|e| StartInternalBitcoindError::CommandError(e.to_string()))?;

        // We've started bitcoind in the background, however it may fail to start for whatever
        // reason. And we need its JSONRPC interface to be available to continue. Thus wait for
        // the interface to be created successfully, regularly checking it did not fail to start.
        let mut try_count = 0;
        loop {
            match process.try_wait() {
                Ok(None) => {}
                Err(e) => log::error!("Error while trying to wait for bitcoind: {e}"),
                Ok(Some(status)) => {
                    log::error!("Bitcoind exited with status '{status}'");
                    return Err(StartInternalBitcoindError::ProcessExited(status));
                }
            }
            match lianad::BitcoinD::new(&config, "internal_bitcoind_start".to_string()) {
                Ok(_) => {
                    log::info!("Bitcoind seems to have successfully started.");
                    return Ok(Self {
                        config,
                        lock: LockFile::create(liana_datadir.bitcoind_directory(), network)
                            .map_err(|e| StartInternalBitcoindError::Lock(format!("{e:?}")))?,
                    });
                }
                Err(lianad::BitcoindError::CookieFile(_)) => {
                    // This is only raised if we're using cookie authentication.
                    // Assume cookie file has not been created yet and try again.
                }
                Err(e) => {
                    if !e.is_transient() && (!e.is_unauthorized() || try_count > 10) {
                        // Non-transient error could happen, e.g., if RPC auth credentials are wrong.
                        // Kill process now in case it's not possible to do via RPC command later.
                        // If the auth credentials are wrong, it is possible that liana-gui is
                        // reading the previous state of the .cookie file and not the new generated
                        // one.
                        if let Err(e) = process.kill() {
                            log::error!("Error trying to kill bitcoind process: '{e}'");
                        }
                        return Err(StartInternalBitcoindError::BitcoinDError(e.to_string()));
                    }
                }
            }
            try_count += 1;
            log::info!("Waiting for bitcoind to start.");
            thread::sleep(time::Duration::from_millis(500));
        }
    }

    /// Stop (internal) bitcoind.
    pub fn stop(self) {
        match self.lock.delete() {
            Err(e) => {
                tracing::error!("Failed to release bitcoind lock: {}", e);
            }
            Ok(false) => {
                info!("Other processes are using internal bitcoind. Process lock has been deleted");
            }
            Ok(true) => {
                match lianad::BitcoinD::new(&self.config, "internal_bitcoind_stop".to_string()) {
                    Ok(bitcoind) => {
                        info!("Stopping internal bitcoind...");
                        bitcoind.stop();
                        info!("Stopped liana managed bitcoind");
                    }
                    Err(e) => {
                        warn!("Could not create interface to internal bitcoind: '{}'.", e);
                    }
                }
            }
        }
    }
}

const LOCK_DIRECTORY_NAME: &str = "locks";

#[derive(Debug, Clone)]
struct LockFile {
    path: PathBuf,
    directory: BitcoindDirectory,
    network: Network,
}

impl LockFile {
    fn create(
        directory: BitcoindDirectory,
        network: Network,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut path = directory.clone().path().to_path_buf();
        path.push(LOCK_DIRECTORY_NAME);
        path.push(network.to_string());
        std::fs::create_dir_all(&path)?;

        path.push(format!(
            "{}-{}.lock",
            std::process::id(),
            now_fallible()?.as_secs()
        ));

        std::fs::File::create(&path)?;
        Ok(Self {
            path,
            directory,
            network,
        })
    }

    // returns true if the lock directory is removed because empty.
    fn delete(self) -> Result<bool, Box<dyn std::error::Error>> {
        std::fs::remove_file(self.path)?;
        if std::fs::read_dir(
            self.directory
                .path()
                .join(LOCK_DIRECTORY_NAME)
                .join(self.network.to_string()),
        )?
        .next()
        .is_none()
        {
            std::fs::remove_dir(
                self.directory
                    .path()
                    .join(LOCK_DIRECTORY_NAME)
                    .join(self.network.to_string()),
            )?;

            if std::fs::read_dir(self.directory.path().join(LOCK_DIRECTORY_NAME))?
                .next()
                .is_none()
            {
                std::fs::remove_dir(self.directory.path().join(LOCK_DIRECTORY_NAME))?;
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// In case of panic, we remove all the bitcoind locks created by the process.
pub fn delete_all_bitcoind_locks_for_process(
    directory: BitcoindDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    let locks_directory = directory.path().join(LOCK_DIRECTORY_NAME);
    if !locks_directory.exists() {
        tracing::debug!("No internal bitcoind locks for the current process");
        return Ok(());
    }
    tracing::info!("Deleting all internal bitcoind locks for the current process");
    let process_prefix = format!("{}-", std::process::id());
    for network_dir in std::fs::read_dir(&locks_directory)? {
        let dir = network_dir?.path();
        for lock_file in std::fs::read_dir(&dir)? {
            let file = lock_file?.path();
            if let Some(name) = file.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(&process_prefix) {
                    std::fs::remove_file(file)?;
                }
            }
        }
        if std::fs::read_dir(&dir)?.next().is_none() {
            std::fs::remove_dir(dir)?;
        }
    }
    if std::fs::read_dir(&locks_directory)?.next().is_none() {
        std::fs::remove_dir(locks_directory)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RpcAuthType {
    CookieFile,
    UserPass,
}

impl fmt::Display for RpcAuthType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            RpcAuthType::CookieFile => write!(f, "Cookie file path"),
            RpcAuthType::UserPass => write!(f, "User and password"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RpcAuthValues {
    pub cookie_path: form::Value<String>,
    pub user: form::Value<String>,
    pub password: form::Value<String>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
    CookieFilePath,
    User,
    Password,
}

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigField::Address => write!(f, "Socket address"),
            ConfigField::CookieFilePath => write!(f, "Cookie file path"),
            ConfigField::User => write!(f, "User"),
            ConfigField::Password => write!(f, "Password"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liana::miniscript::bitcoin::Network;

    const VALID_CONF: &str = "[main]\n\
        rpcport=43345\n\
        port=42355\n\
        prune=15246\n\
        \n\
        [regtest]\n\
        rpcport=34067\n\
        port=45175\n\
        prune=2043\n\
        rpcauth=my_user:my_salt$my_pw_hmac\n";

    // Test the format of the internal bitcoind configuration file.
    #[test]
    fn internal_bitcoind_config() {
        let conf: InternalBitcoindConfig = VALID_CONF.parse().expect("Parsing conf");
        let main_conf = InternalBitcoindNetworkConfig {
            rpc_port: 43345,
            p2p_port: 42355,
            prune: 15246,
            rpc_auth: None,
        };
        let regtest_conf = InternalBitcoindNetworkConfig {
            rpc_port: 34067,
            p2p_port: 45175,
            prune: 2043,
            rpc_auth: Some(RpcAuth {
                user: "my_user".to_string(),
                salt: "my_salt".to_string(),
                password_hmac: "my_pw_hmac".to_string(),
            }),
        };
        assert_eq!(conf.networks.len(), 2);
        assert_eq!(
            conf.networks.get(&Network::Bitcoin).expect("Missing main"),
            &main_conf
        );
        assert_eq!(
            conf.networks
                .get(&Network::Regtest)
                .expect("Missing regtest"),
            &regtest_conf
        );

        let mut conf = InternalBitcoindConfig::new();
        conf.networks.insert(Network::Bitcoin, main_conf);
        conf.networks.insert(Network::Regtest, regtest_conf);
        assert_eq!(conf.to_string(), VALID_CONF);
    }

    #[test]
    fn internal_bitcoind_config_parse_errors() {
        assert!(
            "# a comment\n; another\n[main]\nrpcport=1\nport=2\nprune=3\n"
                .parse::<InternalBitcoindConfig>()
                .is_ok()
        );
        assert_eq!(
            "[main]\nrpcport\n"
                .parse::<InternalBitcoindConfig>()
                .expect_err("Line without separator"),
            InternalBitcoindConfigError::CouldNotParseValue("rpcport".to_string())
        );
        assert_eq!(
            "rpcport=43345\n"
                .parse::<InternalBitcoindConfig>()
                .expect_err("Value outside any section"),
            InternalBitcoindConfigError::UnexpectedSection(
                "General section should be empty".to_string()
            )
        );
        assert!(matches!(
            "[dummynet]\nrpcport=43345\n"
                .parse::<InternalBitcoindConfig>()
                .expect_err("Unknown network section"),
            InternalBitcoindConfigError::UnexpectedSection(_)
        ));
        assert_eq!(
            "[main]\nrpcport=43345\nprune=15246\n"
                .parse::<InternalBitcoindConfig>()
                .expect_err("Missing port key"),
            InternalBitcoindConfigError::KeyNotFound("port".to_string())
        );
    }
}
