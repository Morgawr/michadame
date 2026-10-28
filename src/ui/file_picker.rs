//! File selection inside the existing Deck viewport, including in Game Mode.
use eframe::egui;
use std::path::{Path, PathBuf};

pub struct FilePicker {
    title: String,
    directory: PathBuf,
    path_text: String,
    folders_only: bool,
    extension: Option<&'static str>,
    entries: Vec<(PathBuf, bool)>,
    select: Option<Box<dyn FnOnce(PathBuf) + Send>>,
}

impl FilePicker {
    pub fn new(
        title: &str,
        initial: &Path,
        folders_only: bool,
        extension: Option<&'static str>,
        select: impl FnOnce(PathBuf) + Send + 'static,
    ) -> Self {
        let directory = if initial.is_dir() {
            initial.to_path_buf()
        } else if initial.is_file() {
            initial.parent().unwrap_or(Path::new("/")).to_path_buf()
        } else {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
        };
        let mut picker = Self {
            title: title.into(),
            path_text: directory.to_string_lossy().into_owned(),
            directory,
            folders_only,
            extension,
            entries: Vec::new(),
            select: Some(Box::new(select)),
        };
        picker.refresh();
        picker
    }

    fn refresh(&mut self) {
        self.entries = std::fs::read_dir(&self.directory)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let path = entry.path();
                        let directory = path.is_dir();
                        (directory
                            || (!self.folders_only
                                && self.extension.is_none_or(|ext| {
                                    path.extension()
                                        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
                                })))
                        .then_some((path, directory))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.entries.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
        });
    }

    fn navigate(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.directory = path;
            self.path_text = self.directory.to_string_lossy().into_owned();
            self.refresh();
        }
    }

    /// Returns true when selected or cancelled. Does not open a native window.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let mut selected = None;
        let mut next_directory = None;
        let size =
            (ctx.screen_rect().size() - egui::vec2(32.0, 48.0)).max(egui::vec2(240.0, 200.0));
        egui::Window::new(&self.title)
            .id(egui::Id::new("deck_file_picker"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size(size)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Up").clicked() {
                        next_directory = self.directory.parent().map(Path::to_path_buf);
                    }
                    for (name, path) in [
                        ("Home", std::env::var_os("HOME").map(PathBuf::from)),
                        ("SD cards", Some(PathBuf::from("/run/media"))),
                        ("/", Some(PathBuf::from("/"))),
                    ] {
                        if let Some(path) = path.filter(|p| p.is_dir()) {
                            if ui.button(name).clicked() {
                                next_directory = Some(path);
                            }
                        }
                    }
                    if ui.button("Refresh").clicked() {
                        self.refresh();
                    }
                });
                let edit = ui.add_sized(
                    [ui.available_width(), 40.0],
                    egui::TextEdit::singleline(&mut self.path_text),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let path = PathBuf::from(self.path_text.trim());
                    if path.is_dir() {
                        next_directory = Some(path);
                    } else if !self.folders_only && path.is_file() {
                        selected = Some(path);
                    }
                }
                if self.folders_only && ui.button("Select this folder").clicked() {
                    selected = Some(self.directory.clone());
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("deck_picker_files")
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - 12.0).max(80.0))
                    .show(ui, |ui| {
                        for (path, directory) in &self.entries {
                            let label = format!(
                                "{}{}",
                                if *directory { "▸ " } else { "" },
                                path.file_name().unwrap_or_default().to_string_lossy()
                            );
                            if ui
                                .add_sized([ui.available_width(), 40.0], egui::Button::new(label))
                                .clicked()
                            {
                                if *directory {
                                    next_directory = Some(path.clone());
                                } else {
                                    selected = Some(path.clone());
                                }
                            }
                        }
                    });
            });
        if let Some(directory) = next_directory {
            self.navigate(directory);
        }
        if let Some(path) = selected {
            if let Some(select) = self.select.take() {
                select(path);
            }
            return true;
        }
        !open
    }
}
