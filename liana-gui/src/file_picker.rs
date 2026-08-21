//! In-app modal file picker, used instead of a native file dialog.

use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf, MAIN_SEPARATOR_STR},
};

use iced::widget::{column, row, Space};
use liana_ui::{
    component::{
        button::{btn_cancel, btn_ok, btn_save, EntryWidth},
        form,
        list::{list_entry_row, right_chevron},
        modal::{modal_view, ModalWidth},
        scrollable,
        text::new,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Column, Element, SpaceExt},
};

/// Height of the entry list, so the modal keeps a stable size whatever the directory holds.
const LIST_HEIGHT: u32 = 320;

#[derive(Debug, Clone)]
pub enum Message {
    Enter(PathBuf),
    Parent,
    Select(PathBuf),
    FileNameEdited(String),
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Chosen(PathBuf),
    Cancelled,
}

/// What confirming the picker yields.
#[derive(Debug, Clone)]
enum Mode {
    /// An existing file, optionally restricted to one extension.
    Open {
        extension: Option<String>,
        selected: Option<PathBuf>,
    },
    /// The current directory joined with a typed file name.
    Save { filename: form::Value<String> },
}

#[derive(Debug, Clone)]
struct DirEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct FilePicker {
    directory: PathBuf,
    entries: Vec<DirEntry>,
    /// Why the current directory could not be listed.
    error: Option<String>,
    mode: Mode,
}

impl FilePicker {
    pub fn open(start_dir: PathBuf, extension: Option<String>) -> Self {
        Self::with_mode(
            start_dir,
            Mode::Open {
                extension,
                selected: None,
            },
        )
    }

    pub fn save(start_dir: PathBuf, default_filename: String) -> Self {
        Self::with_mode(
            start_dir,
            Mode::Save {
                filename: form::Value {
                    value: default_filename,
                    warning: None,
                    valid: true,
                },
            },
        )
    }

    /// Where to start browsing when the caller has no better place: the user home on unix,
    /// the roaming application data folder on windows, the filesystem root otherwise.
    pub fn default_dir() -> PathBuf {
        lianad::config::base_config_dir().unwrap_or_else(|| PathBuf::from(MAIN_SEPARATOR_STR))
    }

    fn with_mode(directory: PathBuf, mode: Mode) -> Self {
        let mut picker = Self {
            directory,
            entries: Vec::new(),
            error: None,
            mode,
        };
        picker.reload();
        picker
    }

    pub fn update(&mut self, message: Message) -> Option<Outcome> {
        match message {
            Message::Enter(path) => {
                self.enter(path);
                None
            }
            Message::Parent => {
                if let Some(parent) = self.directory.parent() {
                    self.enter(parent.to_path_buf());
                }
                None
            }
            Message::Select(path) => {
                match &mut self.mode {
                    Mode::Open { selected, .. } => *selected = Some(path),
                    Mode::Save { filename } => filename.value = file_name(&path),
                }
                None
            }
            Message::FileNameEdited(value) => {
                if let Mode::Save { filename } = &mut self.mode {
                    filename.value = value;
                }
                None
            }
            Message::Confirm => self.target().map(Outcome::Chosen),
            Message::Cancel => Some(Outcome::Cancelled),
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let title = match self.mode {
            Mode::Open { .. } => "Select a file",
            Mode::Save { .. } => "Save to file",
        };
        let parent = self.directory.parent().map(|_| Message::Parent);
        let location =
            new::caption(self.directory.display().to_string()).style(theme::text::secondary);
        let error = self
            .error
            .as_ref()
            .map(|e| new::caption(e).style(theme::text::warning));
        let entries = self
            .entries
            .iter()
            .fold(Column::new().spacing(VSpacing::XS), |col, entry| {
                col.push(self.entry_row(entry))
            });
        let listing = scrollable::vertical(entries).height(LIST_HEIGHT);
        let filename = match &self.mode {
            Mode::Save { filename } => Some(form::Form::new(
                "File name",
                filename,
                Message::FileNameEdited,
            )),
            Mode::Open { .. } => None,
        };
        let target = self.target();
        let confirm: Element<Message> = match self.mode {
            Mode::Open { .. } => btn_ok(target.map(|_| Message::Confirm)).into(),
            Mode::Save { .. } => btn_save(target.map(|_| Message::Confirm), true).into(),
        };
        let actions = row![
            Space::fill_width(),
            btn_cancel(Some(Message::Cancel)),
            confirm
        ]
        .spacing(HSpacing::M);
        let content = column![location, error, listing, filename, actions].spacing(VSpacing::M);

        modal_view(
            Some(title),
            parent,
            Some(Message::Cancel),
            ModalWidth::L,
            content,
        )
    }

    fn entry_row<'a>(&'a self, entry: &'a DirEntry) -> Element<'a, Message> {
        let trailing: Option<Element<'a, Message>> = if entry.is_dir {
            Some(right_chevron())
        } else if self.is_selected(&entry.path) {
            Some(icon::check_icon().style(theme::text::success).into())
        } else {
            None
        };
        let message = if entry.is_dir {
            Message::Enter(entry.path.clone())
        } else {
            Message::Select(entry.path.clone())
        };
        list_entry_row(
            None,
            new::b5_medium(&entry.name),
            trailing,
            None,
            EntryWidth::Fill,
            Some(message),
        )
    }

    fn is_selected(&self, path: &Path) -> bool {
        match &self.mode {
            Mode::Open { selected, .. } => selected.as_deref() == Some(path),
            Mode::Save { .. } => false,
        }
    }

    fn enter(&mut self, directory: PathBuf) {
        self.directory = directory;
        if let Mode::Open { selected, .. } = &mut self.mode {
            *selected = None;
        }
        self.reload();
    }

    fn reload(&mut self) {
        let extension = match &self.mode {
            Mode::Open { extension, .. } => extension.as_deref(),
            Mode::Save { .. } => None,
        };
        match read_entries(&self.directory, extension) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.error = Some(e);
            }
        }
    }

    /// The path confirming would yield, or `None` while the choice is incomplete.
    fn target(&self) -> Option<PathBuf> {
        match &self.mode {
            Mode::Open { selected, .. } => selected.clone(),
            Mode::Save { filename } => {
                let name = filename.value.trim();
                (!name.is_empty()).then(|| self.directory.join(name))
            }
        }
    }
}

/// List a directory, keeping every subdirectory but only the files matching `extension`.
/// Directories come first, then files, each sorted by name.
fn read_entries(directory: &Path, extension: Option<&str>) -> Result<Vec<DirEntry>, String> {
    let listing =
        fs::read_dir(directory).map_err(|e| format!("Cannot open {}: {e}", directory.display()))?;
    let mut entries = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|e| format!("Cannot read {}: {e}", directory.display()))?;
        let path = entry.path();
        // Follows symlinks, so a link to a directory is browsable.
        let is_dir = path.is_dir();
        if !is_dir && !matches_extension(&path, extension) {
            continue;
        }
        entries.push(DirEntry {
            name: file_name(&path),
            path,
            is_dir,
        });
    }
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(entries)
}

fn matches_extension(path: &Path, extension: Option<&str>) -> bool {
    match extension {
        Some(extension) => path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case(extension)),
        None => true,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static FIXTURE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A temporary directory holding `a.txt`, `b.dat` and `sub/c.txt`, removed on drop.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let id = FIXTURE_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("liana-file-picker-{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("sub")).unwrap();
            fs::write(root.join("a.txt"), "a").unwrap();
            fs::write(root.join("b.dat"), "b").unwrap();
            fs::write(root.join("sub").join("c.txt"), "c").unwrap();
            Self { root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn names(picker: &FilePicker) -> Vec<String> {
        picker.entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn entering_a_directory_lists_its_content() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::open(fixture.root.clone(), None);
        assert_eq!(names(&picker), vec!["sub", "a.txt", "b.dat"]);

        assert_eq!(
            picker.update(Message::Enter(fixture.root.join("sub"))),
            None
        );
        assert_eq!(picker.directory, fixture.root.join("sub"));
        assert_eq!(names(&picker), vec!["c.txt"]);
    }

    #[test]
    fn parent_goes_up_one_directory() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::open(fixture.root.join("sub"), None);
        assert_eq!(names(&picker), vec!["c.txt"]);

        assert_eq!(picker.update(Message::Parent), None);
        assert_eq!(picker.directory, fixture.root);
        assert_eq!(names(&picker), vec!["sub", "a.txt", "b.dat"]);
    }

    #[test]
    fn extension_filter_hides_other_files_but_keeps_directories() {
        let fixture = Fixture::new();
        let picker = FilePicker::open(fixture.root.clone(), Some("txt".to_string()));
        assert_eq!(names(&picker), vec!["sub", "a.txt"]);
    }

    #[test]
    fn unreadable_directory_reports_an_error() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::open(fixture.root.clone(), None);
        assert_eq!(picker.error, None);

        assert_eq!(
            picker.update(Message::Enter(fixture.root.join("missing"))),
            None
        );
        assert!(picker.error.is_some());
        assert!(names(&picker).is_empty());
    }

    #[test]
    fn open_returns_the_selected_file() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::open(fixture.root.clone(), None);
        assert_eq!(picker.update(Message::Confirm), None);

        assert_eq!(
            picker.update(Message::Select(fixture.root.join("a.txt"))),
            None
        );
        assert_eq!(
            picker.update(Message::Confirm),
            Some(Outcome::Chosen(fixture.root.join("a.txt")))
        );
    }

    #[test]
    fn entering_a_directory_clears_the_open_selection() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::open(fixture.root.clone(), None);
        picker.update(Message::Select(fixture.root.join("a.txt")));

        picker.update(Message::Enter(fixture.root.join("sub")));
        assert_eq!(picker.update(Message::Confirm), None);
    }

    #[test]
    fn save_joins_the_filename_to_the_directory() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::save(fixture.root.clone(), "liana.csv".to_string());
        assert_eq!(
            picker.update(Message::Confirm),
            Some(Outcome::Chosen(fixture.root.join("liana.csv")))
        );

        assert_eq!(
            picker.update(Message::Enter(fixture.root.join("sub"))),
            None
        );
        assert_eq!(
            picker.update(Message::Confirm),
            Some(Outcome::Chosen(fixture.root.join("sub").join("liana.csv")))
        );
    }

    #[test]
    fn save_with_an_empty_filename_yields_no_outcome() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::save(fixture.root.clone(), "liana.csv".to_string());
        assert_eq!(
            picker.update(Message::FileNameEdited("  ".to_string())),
            None
        );
        assert_eq!(picker.update(Message::Confirm), None);
    }

    #[test]
    fn save_selecting_a_file_fills_the_filename() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::save(fixture.root.clone(), "liana.csv".to_string());
        assert_eq!(
            picker.update(Message::Select(fixture.root.join("b.dat"))),
            None
        );
        assert_eq!(
            picker.update(Message::Confirm),
            Some(Outcome::Chosen(fixture.root.join("b.dat")))
        );
    }

    #[test]
    fn cancel_yields_cancelled() {
        let fixture = Fixture::new();
        let mut picker = FilePicker::open(fixture.root.clone(), None);
        assert_eq!(picker.update(Message::Cancel), Some(Outcome::Cancelled));
    }
}
