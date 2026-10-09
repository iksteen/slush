//! Workspace membership lives in tab purposes and round-trips with session layouts.
use super::{App, commands::purpose};
use crate::{
    json_control::{Reply, native_bytes},
    layout::Id,
    path,
    project::{self, Project},
};
use serde_json::json;
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct Opened {
    pub tab: Id,
    pub purpose: Vec<u8>,
    pub path: PathBuf,
    pub created: bool,
    pub tabs: usize,
    pub honoured: usize,
}
pub(super) struct Saved {
    pub path: PathBuf,
    pub purpose: String,
    pub panes: usize,
    pub suspended: usize,
    pub replaced: bool,
}
impl App {
    pub(super) fn projects(&self) -> Vec<Project> {
        project::scan(&self.config.project_roots, self.config.project_depth)
    }
    pub(super) fn find_project(&self, name: impl AsRef<OsStr>) -> Option<Project> {
        project::find(&self.config.project_roots, self.config.project_depth, name)
    }
    pub(super) fn workspace_tab(&self, slug: impl AsRef<[u8]>) -> Option<Id> {
        self.layout
            .tabs
            .iter()
            .find(|t| t.purpose.as_bytes() == slug.as_ref())
            .map(|t| t.id)
    }
    pub(super) fn workspaces_json(&self) -> Reply {
        let entries = self
            .projects()
            .iter()
            .map(|p| {
                Reply::from(json!({"mtime":p.mtime,"tab":self.workspace_tab(&p.slug).unwrap_or(0)}))
                    .field("name", Reply::string_bytes(&p.name))
                    .field("purpose", Reply::string_bytes(&p.slug))
                    .field("path", Reply::native(&p.path))
                    .field(
                        "layout",
                        Reply::native(p.layout.as_deref().unwrap_or_else(|| Path::new(""))),
                    )
            })
            .collect();
        Reply::from(json!({"ok":true,"roots":!self.config.project_roots.is_empty()}))
            .field("workspaces", Reply::Array(entries))
    }
    pub(super) fn workspaces_text(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.config.project_roots.is_empty() {
            out.extend_from_slice(b"no project roots: set project_roots in your config\n");
        }
        for project in self.projects() {
            out.extend_from_slice(&project.name);
            out.extend(std::iter::repeat_n(
                b' ',
                24usize.saturating_sub(project.name.len()),
            ));
            out.extend_from_slice(if project.layout.is_none() {
                b" .git      "
            } else {
                b" layout    "
            });
            out.extend_from_slice(&native_bytes(project.path.as_os_str()));
            if self.workspace_tab(&project.slug).is_some() {
                out.extend_from_slice(b"  (open)");
            }
            out.push(b'\n');
        }
        out
    }
    pub(super) fn open_workspace(
        &mut self,
        name: impl AsRef<OsStr>,
        suspended: bool,
    ) -> Result<Opened, String> {
        let name = name.as_ref();
        let project = self.find_project(name).ok_or_else(|| {
            if self.config.project_roots.is_empty() {
                format!(
                    "no project roots: set project_roots in your config (asked for {})",
                    name.to_string_lossy()
                )
            } else {
                format!("no project called {}", name.to_string_lossy())
            }
        })?;
        let mut opened = Opened {
            tab: 0,
            purpose: project.slug.clone(),
            path: project.path.clone(),
            created: false,
            tabs: 0,
            honoured: 0,
        };
        if let Some(id) = self.workspace_tab(&project.slug) {
            self.layout.current = self.layout.tabs.iter().position(|t| t.id == id).unwrap();
            self.sync_layout();
            opened.tab = id;
            return Ok(opened);
        }
        let before = self.layout.tabs.len();
        let source = if let Some(file) = &project.layout {
            fs::read(file).map_err(|e| e.to_string())?
        } else if let Some(file) = self.config.project_layout.as_deref() {
            fs::read(path::expand_os(file)).map_err(|e| e.to_string())?
        } else {
            b"layout { tab { pane } }".to_vec()
        };
        self.force_suspend = suspended;
        let result = self.apply_layout_text(&source, false, true, Some(&project.path));
        self.force_suspend = false;
        result?;
        for tab in &mut self.layout.tabs[before..] {
            if !tab.purpose.is_empty() && !tab.purpose.starts_with("project:") {
                opened.honoured += 1;
                continue;
            }
            tab.purpose = purpose(&project.slug);
            tab.purpose_locked = true;
            if tab.name.is_empty() {
                tab.name = project.name.clone();
            }
            if opened.tab == 0 {
                opened.tab = tab.id;
            }
            opened.tabs += 1;
        }
        opened.created = true;
        if opened.tab != 0 {
            self.layout.current = self
                .layout
                .tabs
                .iter()
                .position(|t| t.id == opened.tab)
                .unwrap();
        }
        self.sync_layout();
        Ok(opened)
    }
    pub(super) fn close_workspace(&mut self, slug: impl AsRef<[u8]>) -> usize {
        let mut count = 0;
        while let Some(id) = self.workspace_tab(slug.as_ref()) {
            if !self.close_tab(id) {
                break;
            }
            count += 1;
        }
        count
    }
    pub(super) fn save_workspace(
        &mut self,
        id: Id,
        dir: Option<&OsStr>,
        suspend: &str,
        force: bool,
    ) -> Result<Saved, String> {
        if !matches!(suspend, "as-is" | "none" | "commands" | "all") {
            return Err("suspend is as-is, none, commands or all".into());
        }
        let index = if id == 0 {
            Some(self.layout.current)
        } else {
            self.layout.tabs.iter().position(|t| t.id == id)
        }
        .filter(|i| *i < self.layout.tabs.len())
        .ok_or("no such tab")?;
        let tab = &self.layout.tabs[index];
        let project = if let Some(dir) = dir.filter(|s| !s.is_empty()) {
            self.find_project(dir)
        } else {
            self.projects()
                .into_iter()
                .find(|p| p.slug == tab.purpose.as_bytes())
                .or_else(|| {
                    self.panes
                        .get(&tab.focus)?
                        .live_cwd()
                        .and_then(|dir| self.find_project(dir.as_os_str()))
                })
        }
        .ok_or("this tab is not in a project root")?;
        if tab.purpose.starts_with("project:") && tab.purpose.as_bytes() != project.slug {
            return Err(format!(
                "this tab is another project's workspace ({}): save it from its own tab, or pass that tab's id",
                tab.purpose
            ));
        }
        let file = project.path.join(project::LAYOUT_FILE);
        let replaced = project.layout.is_some();
        if replaced && !force {
            return Err(format!(
                "{} already has a layout: pass force to replace it",
                String::from_utf8_lossy(&project.name)
            ));
        }
        let (kdl, panes, suspended) =
            self.dump_layout_for(tab.id, Some(&project.path), suspend, true)?;
        let mut out =
            fs::File::create(&file).map_err(|_| format!("cannot write {}", file.display()))?;
        out.write_all(b"// What this project needs open, written by `save-workspace`.\n// Checked by `slosh --check`; opened by `open-workspace`.\n")
            .and_then(|_| out.write_all(&kdl)).and_then(|_| out.flush())
            .map_err(|_| format!("cannot write {}", file.display()))?;
        let tab = &mut self.layout.tabs[index];
        if tab.purpose.is_empty() || tab.purpose.starts_with("project:") {
            tab.purpose = purpose(&project.slug);
            tab.purpose_locked = true;
            if tab.name.is_empty() {
                tab.name = project.name;
            }
        }
        Ok(Saved {
            path: file,
            purpose: tab.purpose.clone(),
            panes,
            suspended,
            replaced,
        })
    }
}
