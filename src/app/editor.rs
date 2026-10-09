//! The configuration editor is a temporary pane in the existing session.
use super::App;
use crate::{
    config::Config,
    layout::{Direction, Id},
    path,
};
use std::{fs::OpenOptions, io::Write};

impl App {
    pub fn edit_config(&mut self) -> Option<Id> {
        let id = self.layout.focus()?;
        let file = Config::default_path();
        if !std::path::Path::new(&file).exists() {
            let _ = path::mkdirs(path::dir_os(&file));
            // Exclusive creation preserves a config another process just wrote.
            if let Ok(mut out) = OpenOptions::new().write(true).create_new(true).open(&file)
                && out.write_all(&Config::default().render_bytes()).is_ok()
            {
                self.toast("wrote a starting config");
            }
        }
        if !self.layout.split_fits(id, Direction::Rows, &self.config) {
            self.toast("no room to split down");
            return None;
        }
        let requested = self
            .config
            .editor
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(std::ffi::OsStr::to_owned)
            .or_else(|| std::env::var_os("EDITOR").filter(|s| !s.is_empty()));
        let (editor, windowed) = crate::shell::editor(requested);
        if windowed {
            self.toast("no console editor found: opened the config in notepad");
        }
        // The editor may contain shell arguments; quote the native filename.
        let command = crate::shell::editor_command(&editor, &file);
        let argv = crate::shell::command(&command);
        let label = crate::json_control::native_bytes(&command);
        match self.split_with(id, Direction::Rows, false, argv, &label) {
            Ok(Some(fresh)) => {
                self.panes.get_mut(&fresh).unwrap().ephemeral = true;
                Some(fresh)
            }
            Ok(None) => None,
            Err(e) => {
                self.toast(&format!("cannot open editor: {e}"));
                None
            }
        }
    }
}
