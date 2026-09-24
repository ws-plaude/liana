use std::{
    fs::{File, OpenOptions},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use fs2::FileExt;
use iced::{
    widget::{column, row, Space},
    Subscription, Task,
};
use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{
        button::{btn_ok, btn_retry, btn_skip},
        card, installer as installer_layout,
        text::new,
    },
    spacing::{HSpacing, VSpacing},
    widget::{Element, SpaceExt},
    Variant,
};
use rustc_version::Version;

use crate::{
    app::{
        config::Config,
        settings::{
            global::{BitcoindUpgradeSettings, GlobalSettings},
            LianaWalletSettings,
        },
    },
    dir::LianaDirectory,
    download::{self, Progress},
    installer::{
        bitcoind_download_status,
        step::{install_bitcoind, DownloadState},
    },
    node::bitcoind::{self, internal_bitcoind_directory, internal_bitcoind_exe_path, VERSION},
    t,
};

static NEXT_DOWNLOAD_ID: AtomicUsize = AtomicUsize::new(0);

fn seen_version(settings: &BitcoindUpgradeSettings) -> Result<Option<Version>, String> {
    settings
        .highest_liana_version
        .as_deref()
        .map(Version::parse)
        .transpose()
        .map_err(|e| format!("Invalid saved Liana version: {e}"))
}

fn needs_upgrade(
    settings: &BitcoindUpgradeSettings,
    datadir: &LianaDirectory,
) -> Result<bool, String> {
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package version");
    let seen = seen_version(settings)?;
    Ok(seen.is_none_or(|seen| seen < current)
        && !settings
            .skipped_bitcoind_versions
            .iter()
            .any(|v| v == VERSION)
        && !internal_bitcoind_exe_path(datadir, VERSION).is_file())
}

fn read_settings(datadir: &LianaDirectory) -> Result<BitcoindUpgradeSettings, String> {
    let mut settings = BitcoindUpgradeSettings::default();
    GlobalSettings::update(
        &GlobalSettings::path(datadir),
        |global| {
            settings = global.bitcoind_upgrade.clone().unwrap_or_default();
        },
        false,
    )?;
    Ok(settings)
}

fn save_settings(datadir: &LianaDirectory, skip: bool) -> Result<(), String> {
    let settings = read_settings(datadir)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package version");
    let seen = seen_version(&settings)?;
    GlobalSettings::update(
        &GlobalSettings::path(datadir),
        |global| {
            let upgrade = global.bitcoind_upgrade.get_or_insert_with(Default::default);
            if seen.as_ref().is_none_or(|seen| seen < &current) {
                upgrade.highest_liana_version = Some(current.to_string());
            }
            if skip
                && !upgrade
                    .skipped_bitcoind_versions
                    .iter()
                    .any(|v| v == VERSION)
            {
                upgrade.skipped_bitcoind_versions.push(VERSION.to_string());
            }
        },
        true,
    )
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // Signal 0 checks whether the process exists without sending a signal.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let mut exit_code = 0;
    let active = unsafe { GetExitCodeProcess(handle, &mut exit_code) } == 0
        || exit_code == STILL_ACTIVE as u32;
    unsafe { CloseHandle(handle) };
    active
}

fn managed_node_in_use(datadir: &LianaDirectory) -> Result<bool, String> {
    let locks = internal_bitcoind_directory(datadir).join("locks");
    let networks = match std::fs::read_dir(&locks) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.to_string()),
    };
    for network in networks {
        for marker in std::fs::read_dir(network.map_err(|e| e.to_string())?.path())
            .map_err(|e| e.to_string())?
        {
            let marker = marker.map_err(|e| e.to_string())?;
            let name = marker.file_name();
            let name = name.to_string_lossy();
            if let Some(pid) = name.split('-').next().and_then(|pid| pid.parse().ok()) {
                if pid == 0 {
                    continue;
                }
                if pid_alive(pid) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[derive(Debug, Clone)]
pub enum Message {
    Check,
    Retry,
    Progress(Result<Progress, download::DownloadError>),
    Installed(Result<(), String>),
    Skip,
    Continue,
}

#[derive(Debug)]
enum Stage {
    Waiting,
    Downloading(f32),
    Installing,
    Installed,
    Failed(String),
}

pub struct Upgrade {
    pub datadir: LianaDirectory,
    pub config: Config,
    pub network: Network,
    pub wallet: LianaWalletSettings,
    lock: Option<File>,
    stage: Stage,
    download_id: usize,
    skip_attempt: bool,
    save_failed: bool,
}

impl Upgrade {
    fn uses_managed_node(&self) -> bool {
        self.wallet.remote_backend_auth.is_none()
            && self
                .wallet
                .start_internal_bitcoind
                .unwrap_or(self.config.start_internal_bitcoind)
    }

    pub fn new(
        datadir: LianaDirectory,
        config: Config,
        network: Network,
        wallet: LianaWalletSettings,
    ) -> (Self, Task<Message>) {
        (
            Self {
                datadir,
                config,
                network,
                wallet,
                lock: None,
                stage: Stage::Waiting,
                download_id: NEXT_DOWNLOAD_ID.fetch_add(1, Ordering::Relaxed),
                skip_attempt: false,
                save_failed: false,
            },
            Task::perform(async {}, |_| Message::Check),
        )
    }

    pub fn take_lock(&mut self) -> Option<File> {
        self.lock.take()
    }

    pub fn set_save_error(&mut self, error: String) {
        self.stage = Stage::Failed(error);
        self.save_failed = true;
    }

    fn check(&mut self) -> Task<Message> {
        if self.lock.is_none() {
            let directory = internal_bitcoind_directory(&self.datadir);
            if let Err(e) = std::fs::create_dir_all(&directory) {
                self.stage = Stage::Failed(e.to_string());
                return Task::none();
            }
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(directory.join("upgrade.lock"))
            {
                Ok(file) => file,
                Err(e) => {
                    self.stage = Stage::Failed(e.to_string());
                    return Task::none();
                }
            };
            match file.try_lock_exclusive() {
                Ok(()) => self.lock = Some(file),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.skip_attempt = true;
                    return Task::none();
                }
                Err(e) => {
                    self.stage = Stage::Failed(e.to_string());
                    return Task::none();
                }
            }
        }
        if self.skip_attempt || !self.uses_managed_node() {
            return Task::perform(async {}, |_| Message::Continue);
        }
        let eligibility = read_settings(&self.datadir)
            .and_then(|settings| needs_upgrade(&settings, &self.datadir));
        match eligibility {
            Ok(false) => Task::perform(async {}, |_| Message::Continue),
            Ok(true) => match managed_node_in_use(&self.datadir) {
                Ok(true) => {
                    tracing::info!("Managed Bitcoin Core is in use; deferring upgrade");
                    self.skip_attempt = true;
                    Task::perform(async {}, |_| Message::Continue)
                }
                Ok(false) => {
                    self.stage = Stage::Downloading(0.0);
                    Task::none()
                }
                Err(e) => {
                    self.stage = Stage::Failed(e);
                    Task::none()
                }
            },
            Err(e) => {
                self.stage = Stage::Failed(e);
                Task::none()
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Check if matches!(self.stage, Stage::Waiting) => self.check(),
            Message::Retry if matches!(self.stage, Stage::Failed(_)) => {
                self.stage = Stage::Waiting;
                self.check()
            }
            Message::Progress(Ok(Progress::Downloading(progress)))
                if matches!(self.stage, Stage::Downloading(_)) =>
            {
                self.stage = Stage::Downloading(progress);
                Task::none()
            }
            Message::Progress(Ok(Progress::Finished(bytes)))
                if matches!(self.stage, Stage::Downloading(_)) =>
            {
                self.stage = Stage::Installing;
                let directory = internal_bitcoind_directory(&self.datadir);
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let staging = directory.join(format!("upgrade-{}", std::process::id()));
                            std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
                            let result = install_bitcoind(&staging, &bytes)
                                .map_err(|e| e.to_string())
                                .and_then(|()| {
                                    let executable = staging
                                        .join(format!("bitcoin-{VERSION}"))
                                        .join("bin")
                                        .join(if cfg!(windows) {
                                            "bitcoind.exe"
                                        } else {
                                            "bitcoind"
                                        });
                                    if !executable.is_file() {
                                        return Err(t!("installer-bitcoind-executable-not-found"));
                                    }
                                    std::fs::rename(
                                        staging.join(format!("bitcoin-{VERSION}")),
                                        directory.join(format!("bitcoin-{VERSION}")),
                                    )
                                    .map_err(|e| e.to_string())
                                });
                            if let Err(e) = std::fs::remove_dir_all(&staging) {
                                tracing::warn!(
                                    "Could not remove Bitcoin Core staging directory: {e}"
                                );
                            }
                            result
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|result| result)
                    },
                    Message::Installed,
                )
            }
            Message::Progress(Err(e)) if matches!(self.stage, Stage::Downloading(_)) => {
                self.stage = Stage::Failed(e.to_string());
                Task::none()
            }
            Message::Installed(Ok(())) if matches!(self.stage, Stage::Installing) => {
                if internal_bitcoind_exe_path(&self.datadir, VERSION).is_file() {
                    self.stage = Stage::Installed;
                } else {
                    self.stage = Stage::Failed(t!("installer-bitcoind-executable-not-found"));
                }
                Task::none()
            }
            Message::Installed(Err(e)) if matches!(self.stage, Stage::Installing) => {
                self.stage = Stage::Failed(e);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub fn finish(&self, skipped: bool) -> Result<(), String> {
        if self.uses_managed_node()
            && self.lock.is_some()
            && !self.skip_attempt
            && (!matches!(self.stage, Stage::Failed(_)) || self.save_failed)
        {
            save_settings(&self.datadir, skipped)
        } else {
            Ok(())
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match self.stage {
            Stage::Waiting => iced::time::every(Duration::from_secs(1)).map(|_| Message::Check),
            Stage::Downloading(_) => {
                download::file(("upgrade", self.download_id), bitcoind::download_url())
                    .map(|(_, progress)| Message::Progress(progress))
            }
            _ => Subscription::none(),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let title = match self.stage {
            Stage::Waiting => t!("bitcoind-upgrade-waiting"),
            _ => t!("bitcoind-upgrade-title", version = VERSION),
        };
        let status: Element<'_, Message> = match &self.stage {
            Stage::Downloading(progress) => bitcoind_download_status(&DownloadState::Downloading {
                progress: *progress,
            }),
            Stage::Installing => new::caption(t!("installer-installing-bitcoind")).into(),
            Stage::Installed => new::caption(t!("installer-installation-complete")).into(),
            Stage::Failed(e) => card::invalid(new::caption(e)).into(),
            _ => new::caption(title.clone()).into(),
        };
        let retry = matches!(self.stage, Stage::Failed(_)).then(|| btn_retry(Some(Message::Retry)));
        let ok = matches!(self.stage, Stage::Installed).then(|| btn_ok(Some(Message::Continue)));
        let skip = (!matches!(self.stage, Stage::Waiting | Stage::Installing))
            .then(|| btn_skip(Some(Message::Skip)));
        let actions = row![Space::fill_width(), retry, skip, ok].spacing(HSpacing::M);
        let content = column![status, actions].spacing(VSpacing::XL);
        installer_layout::layout(
            installer_layout::LayoutConfig {
                variant: Variant::Liana,
                network: self.network,
                email: None,
                is_ws_admin: false,
                nav_bar: installer_layout::NavBar::StepTitle {
                    progress: (1, 1),
                    title,
                    previous_message: None,
                },
                content_width: 800.0,
            },
            content,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        app::settings::global::{BitcoindUpgradeSettings, GlobalSettings, WindowConfig},
        dir::LianaDirectory,
        gui::bitcoind_upgrade::{managed_node_in_use, needs_upgrade, read_settings, save_settings},
        node::bitcoind::{internal_bitcoind_exe_path, VERSION},
    };
    use rustc_version::Version;
    use std::fs;

    #[test]
    fn new_release_checks_managed_node_once() {
        let directory = LianaDirectory::new(std::env::temp_dir().join(format!(
            "liana-upgrade-test-{}-{}",
            std::process::id(),
            "version"
        )));
        let settings = BitcoindUpgradeSettings::default();
        assert!(needs_upgrade(&settings, &directory).expect("valid version"));

        let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("valid package version");
        let previous = format!("{}.{}.{}", current.major - 1, current.minor, current.patch);
        let settings = BitcoindUpgradeSettings {
            highest_liana_version: Some(previous),
            ..settings
        };
        assert!(needs_upgrade(&settings, &directory).expect("valid version"));

        let settings = BitcoindUpgradeSettings {
            highest_liana_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..settings
        };
        assert!(!needs_upgrade(&settings, &directory).expect("valid version"));

        let settings = BitcoindUpgradeSettings {
            highest_liana_version: None,
            skipped_bitcoind_versions: vec![VERSION.to_string()],
        };
        assert!(!needs_upgrade(&settings, &directory).expect("valid version"));

        let invalid = BitcoindUpgradeSettings {
            highest_liana_version: Some("not-a-version".to_string()),
            ..BitcoindUpgradeSettings::default()
        };
        assert!(needs_upgrade(&invalid, &directory).is_err());
    }

    #[test]
    fn installed_and_skipped_versions() {
        let path = std::env::temp_dir().join(format!(
            "liana-upgrade-test-{}-{}",
            std::process::id(),
            "installed"
        ));
        let directory = LianaDirectory::new(path.clone());
        let executable = internal_bitcoind_exe_path(&directory, VERSION);
        fs::create_dir_all(executable.parent().expect("binary directory"))
            .expect("create directory");
        fs::write(&executable, b"installed").expect("create binary");
        assert!(
            !needs_upgrade(&BitcoindUpgradeSettings::default(), &directory).expect("valid version")
        );
        fs::remove_file(&executable).expect("remove binary");

        let global_path = GlobalSettings::path(&directory);
        let window = WindowConfig {
            width: 1200.0,
            height: 700.0,
        };
        GlobalSettings::update_window_config(&global_path, &window).expect("save window config");
        save_settings(&directory, true).expect("save skipped version");
        let settings = read_settings(&directory).expect("read settings");
        assert_eq!(
            settings.highest_liana_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(settings.skipped_bitcoind_versions, vec![VERSION]);
        assert!(!needs_upgrade(&settings, &directory).expect("valid version"));
        assert_eq!(
            GlobalSettings::load_window_config(&global_path),
            Some(window)
        );
        assert!(!path.join("bitcoind/upgrade.json").exists());
        fs::remove_dir_all(path).expect("remove test directory");
    }

    #[test]
    fn active_and_stale_wallet_markers() {
        let path = std::env::temp_dir().join(format!(
            "liana-upgrade-test-{}-{}",
            std::process::id(),
            "markers"
        ));
        let directory = LianaDirectory::new(path.clone());
        let locks = path.join("bitcoind/locks/bitcoin");
        fs::create_dir_all(&locks).expect("create lock directory");
        fs::write(locks.join(format!("{}-1.lock", std::process::id())), b"")
            .expect("create marker");
        assert!(managed_node_in_use(&directory).expect("check markers"));
        fs::remove_file(locks.join(format!("{}-1.lock", std::process::id())))
            .expect("remove marker");
        fs::write(locks.join("99999999-1.lock"), b"").expect("create stale marker");
        assert!(!managed_node_in_use(&directory).expect("check stale marker"));
        fs::remove_dir_all(path).expect("remove test directory");
    }
}
