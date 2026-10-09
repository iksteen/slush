//! Declarative session trees; files resolve working directories against their location.
use super::{App, commands::purpose};
use crate::{
    json_control::{native_bytes, os_string},
    kdl,
    layout::{Direction, Id, Kind, WEIGHT_UNIT},
    pane::Pane,
    path,
    shader::Rect,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
struct Launch {
    id: Id,
    argv: Vec<OsString>,
    cwd: Option<PathBuf>,
    suspended: bool,
    label: Vec<u8>,
}
impl App {
    pub fn apply_layout_file(
        &mut self,
        file: impl AsRef<Path>,
        replace: bool,
        focus: bool,
    ) -> Result<(), String> {
        let file = path::expand_os(file.as_ref());
        let text = std::fs::read(&file).map_err(|_| format!("{}: cannot read", file.display()))?;
        self.apply_layout_text(&text, replace, focus, Some(path::dir_os(&file)))
    }
    pub fn apply_layout_text(
        &mut self,
        text: impl AsRef<[u8]>,
        replace: bool,
        take_focus: bool,
        base: Option<&Path>,
    ) -> Result<(), String> {
        let root = kdl::parse_native(text).map_err(|e| e.to_string())?;
        let lay = root.child("layout").unwrap_or(&root);
        let tabs: Vec<_> = lay.kids.iter().filter(|n| n.name == b"tab").collect();
        if tabs.is_empty() {
            for n in &root.kids {
                if self.config.is_setting(&String::from_utf8_lossy(&n.name)) {
                    return Err(format!(
                        "this is a config, not a layout: `{}` is a setting",
                        String::from_utf8_lossy(&n.name)
                    ));
                }
            }
            return Err("layout declares no tabs".into());
        }
        let before = self.layout.tabs.len();
        let old_view = self.layout.current;
        let old_tab_ids: Vec<_> = self.layout.tabs.iter().map(|t| t.id).collect();
        let mut launches = Vec::new();
        let mut selected = None;
        for tab in tabs {
            let mut restore = None;
            let tree = self.build_layout_node(tab, base, base, &mut launches, &mut restore);
            let tid = self.layout.add_tab(tree, tab.prop("name", b""));
            let ti = self.layout.tabs.len() - 1;
            if let Some(id) = restore {
                self.layout.tabs[ti].focus = id;
            }
            if let Some(p) = tab.property("purpose") {
                let t = &mut self.layout.tabs[ti];
                t.purpose = purpose(p);
                t.purpose_locked = true;
            }
            if tab.prop_bool("active", false) {
                selected = Some(tid);
            }
            self.sync_tab(ti);
        }
        let mut failure = None;
        for launch in launches {
            let r = self.layout.nodes[&launch.id].content;
            match Pane::new(
                launch.id,
                launch.argv,
                self.size(r.w, r.h),
                launch.cwd.as_deref(),
                launch.suspended,
                &launch.label,
                self.session.as_deref(),
                &self.config,
            ) {
                Ok(p) => {
                    self.panes.insert(launch.id, p);
                }
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(e) = failure {
            while self.layout.tabs.len() > before {
                let root = self.layout.tabs.last().unwrap().root;
                for id in self.layout.remove(root) {
                    self.panes.remove(&id);
                }
            }
            self.layout.current = old_view;
            self.sync_layout();
            return Err(format!("cannot create panes: {e}"));
        }
        if replace {
            for id in old_tab_ids {
                self.close_tab(id);
            }
        }
        self.layout.current = if let Some(tid) = selected {
            self.layout.tabs.iter().position(|t| t.id == tid).unwrap()
        } else if take_focus {
            if replace { 0 } else { before }
        } else {
            old_view.min(self.layout.tabs.len() - 1)
        };
        self.sync_layout();
        Ok(())
    }
    fn build_layout_node(
        &mut self,
        node: &kdl::NativeNode,
        inherited: Option<&Path>,
        base: Option<&Path>,
        launches: &mut Vec<Launch>,
        restore: &mut Option<Id>,
    ) -> Id {
        let cwd = node
            .property("cwd")
            .map(|own| path::resolve_os(Path::new(&*os_string(own)), base))
            .or_else(|| inherited.map(Path::to_owned));
        let kids: Vec<_> = node.kids.iter().filter(|n| n.name == b"pane").collect();
        let id = self.layout.leaf();
        let weight = node.prop_int("weight", WEIGHT_UNIT) as i32 as i64;
        self.layout.nodes.get_mut(&id).unwrap().weight =
            if weight < 150 { WEIGHT_UNIT } else { weight };
        if !kids.is_empty() {
            let children: Vec<_> = kids
                .into_iter()
                .map(|kid| self.build_layout_node(kid, cwd.as_deref(), base, launches, restore))
                .collect();
            if children.len() == 1 {
                let child = children[0];
                self.layout.nodes.remove(&id);
                return child;
            }
            for child in &children {
                self.layout.nodes.get_mut(child).unwrap().parent = Some(id);
            }
            self.layout.nodes.get_mut(&id).unwrap().kind = Kind::Split {
                direction: if node.prop("split", b"cols") == b"rows" {
                    Direction::Rows
                } else {
                    Direction::Cols
                },
                children,
            };
            return id;
        }
        let n = self.layout.nodes.get_mut(&id).unwrap();
        if let Some(p) = node.property("purpose") {
            n.purpose = purpose(p);
            n.purpose_locked = true;
        }
        if node.prop_bool("floating", false) {
            n.floating = true;
            self.layout.raise_seq += 1;
            n.raised = self.layout.raise_seq;
            let w = node.prop_int("w", 0);
            let h = node.prop_int("h", 0);
            if w > 0 && h > 0 {
                let coord = |key| {
                    let v = node.prop_int(key, -1);
                    if v < 0 { u16::MAX } else { v as u16 }
                };
                n.float_rect = Rect {
                    x: coord("x"),
                    y: coord("y"),
                    w: w as u16,
                    h: h as u16,
                };
            }
        }
        if node.prop_bool("focus", false) {
            *restore = Some(id);
        }
        let command = node.property("command");
        launches.push(Launch {
            id,
            argv: command.map_or_else(
                || self.default_argv(),
                |bytes| crate::shell::command(&*os_string(bytes)),
            ),
            cwd,
            suspended: self.force_suspend || node.prop_bool("suspended", false),
            label: command.unwrap_or_default().to_vec(),
        });
        id
    }
}

fn quoted(out: &mut Vec<u8>, key: &str, value: impl AsRef<[u8]>) {
    let value = value.as_ref();
    if value.is_empty() {
        return;
    }
    out.push(b' ');
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(b"=\"");
    for byte in value.iter().copied().filter(|b| *b >= 32) {
        if matches!(byte, b'"' | b'\\') {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out.push(b'"');
}

impl App {
    pub fn dump_layout(
        &self,
        tab_id: Id,
        base: Option<&str>,
        suspend: &str,
    ) -> Result<(Vec<u8>, usize, usize), String> {
        self.dump_layout_for(tab_id, base.map(Path::new), suspend, false)
    }
    pub(super) fn dump_layout_for(
        &self,
        tab_id: Id,
        base: Option<&Path>,
        suspend: &str,
        for_project: bool,
    ) -> Result<(Vec<u8>, usize, usize), String> {
        if !matches!(suspend, "as-is" | "none" | "commands" | "all") {
            return Err("suspend is as-is, none, commands or all".into());
        }
        if tab_id != 0 && !self.layout.tabs.iter().any(|t| t.id == tab_id) {
            return Err("no such tab".into());
        }
        let mut out = b"layout {\n".to_vec();
        let mut panes = 0;
        let mut sleeping = 0;
        for (i, t) in self
            .layout
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| tab_id == 0 || t.id == tab_id)
        {
            out.extend_from_slice(b"    tab");
            quoted(&mut out, "name", &t.name);
            if !(for_project && t.purpose.starts_with("project:")) {
                quoted(&mut out, "purpose", &t.purpose);
            }
            if tab_id == 0 && i == self.layout.current {
                out.extend_from_slice(b" active=true");
            }
            let root = &self.layout.nodes[&t.root];
            if let Some(dir) = root.direction() {
                quoted(
                    &mut out,
                    "split",
                    if dir == Direction::Rows {
                        "rows"
                    } else {
                        "cols"
                    },
                );
            }
            out.extend_from_slice(b" {\n");
            let children = if root.kind == Kind::Leaf {
                vec![t.root]
            } else {
                root.children().to_vec()
            };
            for id in children {
                let (n, s) = self.dump_node(id, t.focus, 2, base, suspend, &mut out);
                panes += n;
                sleeping += s;
            }
            out.extend_from_slice(b"    }\n");
        }
        out.extend_from_slice(b"}\n");
        Ok((out, panes, sleeping))
    }
    fn dump_node(
        &self,
        id: Id,
        focus: Id,
        depth: usize,
        base: Option<&Path>,
        suspend: &str,
        out: &mut Vec<u8>,
    ) -> (usize, usize) {
        let n = &self.layout.nodes[&id];
        let pad = " ".repeat((depth * 4).min(60));
        out.extend_from_slice(pad.as_bytes());
        out.extend_from_slice(b"pane");
        if n.parent.is_some() {
            out.extend_from_slice(format!(" weight={}", n.weight).as_bytes());
        }
        if let Some(dir) = n.direction() {
            quoted(
                out,
                "split",
                if dir == Direction::Rows {
                    "rows"
                } else {
                    "cols"
                },
            );
            out.extend_from_slice(b" {\n");
            let mut count = (0, 0);
            for child in n.children() {
                let got = self.dump_node(*child, focus, depth + 1, base, suspend, out);
                count.0 += got.0;
                count.1 += got.1;
            }
            out.extend_from_slice(pad.as_bytes());
            out.extend_from_slice(b"}\n");
            return count;
        }
        let p = &self.panes[&id];
        if let Some(live) = p.live_cwd() {
            let cwd = if let Some(start) = &p.cwd
                && std::fs::canonicalize(start).ok() == std::fs::canonicalize(&live).ok()
            {
                start
            } else {
                &live
            };
            quoted(
                out,
                "cwd",
                native_bytes(path::relative_os(cwd, base).as_os_str()),
            );
        }
        let command = if p.ephemeral {
            None
        } else if !p.label.is_empty() {
            Some(p.label.clone())
        } else {
            p.foreground_command()
        };
        if let Some(command) = &command {
            quoted(out, "command", command);
        }
        quoted(out, "purpose", &n.purpose);
        let sleeping = match suspend {
            "none" => false,
            "all" => true,
            "commands" => command.as_ref().is_some_and(|c| !c.is_empty()),
            _ => p.suspended,
        };
        if sleeping {
            out.extend_from_slice(b" suspended=true");
        }
        if n.floating {
            out.extend_from_slice(b" floating=true");
            if n.float_rect.w != 0 {
                if n.float_rect.x != u16::MAX {
                    out.extend_from_slice(format!(" x={}", n.float_rect.x).as_bytes());
                }
                if n.float_rect.y != u16::MAX {
                    out.extend_from_slice(format!(" y={}", n.float_rect.y).as_bytes());
                }
                out.extend_from_slice(
                    format!(" w={} h={}", n.float_rect.w, n.float_rect.h).as_bytes(),
                );
            }
        }
        if id == focus {
            out.extend_from_slice(b" focus=true");
        }
        out.push(b'\n');
        (1, usize::from(sleeping))
    }
}
