//! Weighted n-ary pane trees. Geometry is derived anew for each frame.
use crate::{
    config::{Config, Setting},
    shader::Rect,
};
use std::collections::BTreeMap;
pub type Id = u32;
pub const WEIGHT_UNIT: i64 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Cols,
    Rows,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Leaf,
    Split {
        direction: Direction,
        children: Vec<Id>,
    },
}
#[derive(Clone, Debug)]
pub struct Node {
    pub id: Id,
    pub parent: Option<Id>,
    pub kind: Kind,
    pub weight: i64,
    pub rect: Rect,
    pub content: Rect,
    pub minimized: bool,
    pub floating: bool,
    pub float_rect: Rect,
    pub raised: u32,
    pub collapsed: bool,
    pub hidden: bool,
    pub purpose: String,
    pub purpose_locked: bool,
}
impl Node {
    fn leaf(id: Id) -> Self {
        Self {
            id,
            parent: None,
            kind: Kind::Leaf,
            weight: WEIGHT_UNIT,
            rect: Rect::default(),
            content: Rect::default(),
            minimized: false,
            floating: false,
            float_rect: Rect::default(),
            raised: 0,
            collapsed: false,
            hidden: false,
            purpose: String::new(),
            purpose_locked: false,
        }
    }
    pub fn children(&self) -> &[Id] {
        match &self.kind {
            Kind::Leaf => &[],
            Kind::Split { children, .. } => children,
        }
    }
    pub fn direction(&self) -> Option<Direction> {
        match self.kind {
            Kind::Split { direction, .. } => Some(direction),
            _ => None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: Id,
    pub root: Id,
    pub focus: Id,
    pub zoom: Option<Id>,
    pub name: Vec<u8>,
    pub purpose: String,
    pub purpose_locked: bool,
    pub min_bar: Rect,
    pub flattened: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub nodes: BTreeMap<Id, Node>,
    pub tabs: Vec<Tab>,
    pub current: usize,
    next_id: Id,
    next_tab: Id,
    pub(crate) raise_seq: u32,
}
impl Layout {
    pub fn leaf(&mut self) -> Id {
        self.next_id += 1;
        let id = self.next_id;
        self.nodes.insert(id, Node::leaf(id));
        id
    }
    pub fn add_tab(&mut self, root: Id, name: impl AsRef<[u8]>) -> Id {
        self.next_tab += 1;
        let id = self.next_tab;
        self.tabs.push(Tab {
            id,
            root,
            focus: self.first_leaf(root),
            zoom: None,
            name: name.as_ref()[..name.as_ref().len().min(63)].to_vec(),
            purpose: String::new(),
            purpose_locked: false,
            min_bar: Rect::default(),
            flattened: false,
        });
        id
    }
    pub fn leaves(&self, root: Id) -> Vec<Id> {
        let mut result = Vec::new();
        self.collect(root, &mut result);
        result
    }
    fn collect(&self, id: Id, out: &mut Vec<Id>) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        if node.kind == Kind::Leaf {
            out.push(id);
        } else {
            for child in node.children() {
                self.collect(*child, out);
            }
        }
    }
    pub fn first_leaf(&self, mut id: Id) -> Id {
        while let Some(node) = self.nodes.get(&id) {
            match node.children().first() {
                Some(child) => id = *child,
                None => break,
            }
        }
        id
    }
    pub fn tab_of(&self, id: Id) -> Option<usize> {
        let mut root = id;
        while let Some(parent) = self.nodes.get(&root)?.parent {
            root = parent;
        }
        self.tabs.iter().position(|tab| tab.root == root)
    }
    pub fn focus(&self) -> Option<Id> {
        self.tabs
            .get(self.current)
            .map(|t| t.focus)
            .filter(|id| self.nodes.contains_key(id))
    }
    pub fn focus_pane(&mut self, id: Id) -> bool {
        let Some(index) = self.tab_of(id) else {
            return false;
        };
        if self.nodes[&id].kind != Kind::Leaf {
            return false;
        }
        self.current = index;
        self.tabs[index].focus = id;
        true
    }
    pub fn insert_beside(&mut self, leaf: Id, new: Id, direction: Direction, before: bool) -> bool {
        let Some(ti) = self.tab_of(leaf) else {
            return false;
        };
        if self.nodes[&leaf].kind != Kind::Leaf || !self.nodes.contains_key(&new) {
            return false;
        }
        self.tabs[ti].zoom = None;
        let parent = self.nodes[&leaf].parent;
        if let Some(parent) = parent.filter(|p| self.nodes[p].direction() == Some(direction)) {
            let p = self.nodes.get_mut(&parent).unwrap();
            let Kind::Split { children, .. } = &mut p.kind else {
                unreachable!()
            };
            let at = children.iter().position(|id| *id == leaf).unwrap() + usize::from(!before);
            children.insert(at, new);
            self.nodes.get_mut(&new).unwrap().parent = Some(parent);
        } else {
            self.next_id += 1;
            let split = self.next_id;
            let mut node = Node::leaf(split);
            node.parent = parent;
            node.weight = self.nodes[&leaf].weight;
            node.kind = Kind::Split {
                direction,
                children: if before {
                    vec![new, leaf]
                } else {
                    vec![leaf, new]
                },
            };
            self.nodes.insert(split, node);
            if let Some(parent) = parent {
                self.replace_child(parent, leaf, split);
            } else {
                self.tabs[ti].root = split;
            }
            for id in [leaf, new] {
                let n = self.nodes.get_mut(&id).unwrap();
                n.parent = Some(split);
                n.weight = WEIGHT_UNIT;
            }
        }
        true
    }
    fn replace_child(&mut self, parent: Id, old: Id, new: Id) {
        if let Kind::Split { children, .. } = &mut self.nodes.get_mut(&parent).unwrap().kind
            && let Some(child) = children.iter_mut().find(|id| **id == old)
        {
            *child = new;
        }
    }
    /// Detach a leaf without touching the owning application's PTY or terminal.
    fn detach_leaf(&mut self, id: Id) -> bool {
        let Some(mut leaf) = self
            .nodes
            .get(&id)
            .filter(|n| n.kind == Kind::Leaf)
            .cloned()
        else {
            return false;
        };
        if self.tab_of(id).is_none() {
            return false;
        }
        self.remove(id);
        leaf.parent = None;
        leaf.minimized = false;
        leaf.floating = false;
        self.nodes.insert(id, leaf);
        true
    }
    pub fn move_leaf(
        &mut self,
        id: Id,
        destination: Id,
        beside: Id,
        direction: Direction,
        take_focus: bool,
    ) -> bool {
        let Some(source) = self.tab_of(id) else {
            return false;
        };
        let Some(dest) = self.tabs.iter().position(|t| t.id == destination) else {
            return false;
        };
        if source == dest || self.nodes[&id].kind != Kind::Leaf {
            return false;
        }
        let host = if beside == 0 {
            self.tabs[dest].focus
        } else {
            beside
        };
        if self.tab_of(host) != Some(dest) || self.nodes[&host].kind != Kind::Leaf {
            return false;
        }
        let watching = self.tabs[self.current].id;
        let old_focus = self.tabs[dest].focus;
        if !self.detach_leaf(id) {
            return false;
        }
        self.insert_beside(host, id, direction, false);
        let dest = self.tabs.iter().position(|t| t.id == destination).unwrap();
        self.tabs[dest].focus = if take_focus { id } else { old_focus };
        if let Some(index) = self.tabs.iter().position(|t| t.id == watching) {
            self.current = index;
        }
        true
    }
    pub fn move_leaf_to_new_tab(&mut self, id: Id, name: impl AsRef<[u8]>) -> Option<Id> {
        if self.nodes.get(&id)?.kind != Kind::Leaf || self.nodes[&id].parent.is_none() {
            return None;
        }
        let watching = self.tabs.get(self.current)?.id;
        if !self.detach_leaf(id) {
            return None;
        }
        self.nodes.get_mut(&id).unwrap().weight = WEIGHT_UNIT;
        let tab = self.add_tab(id, name);
        if let Some(index) = self.tabs.iter().position(|t| t.id == watching) {
            self.current = index;
        }
        Some(tab)
    }
    pub fn drop_leaf(&mut self, id: Id, host: Id, direction: Direction, before: bool) -> bool {
        let Some(tab) = self.tab_of(host) else {
            return false;
        };
        if id == host
            || self.tab_of(id) != Some(tab)
            || self.nodes[&id].floating
            || self.nodes[&host].floating
            || self.nodes[&id].kind != Kind::Leaf
            || self.nodes[&host].kind != Kind::Leaf
        {
            return false;
        }
        if !self.detach_leaf(id) {
            return false;
        }
        self.insert_beside(host, id, direction, before);
        self.focus_pane(id);
        true
    }
    pub fn swap_leaves(&mut self, a: Id, b: Id) -> bool {
        if a == b {
            return false;
        }
        let Some(x) = self.nodes.get(&a).filter(|n| n.kind == Kind::Leaf).cloned() else {
            return false;
        };
        let Some(y) = self.nodes.get(&b).filter(|n| n.kind == Kind::Leaf).cloned() else {
            return false;
        };
        let (Some(px), Some(py)) = (x.parent, y.parent) else {
            return false;
        };
        let ix = self.nodes[&px]
            .children()
            .iter()
            .position(|id| *id == a)
            .unwrap();
        let iy = self.nodes[&py]
            .children()
            .iter()
            .position(|id| *id == b)
            .unwrap();
        if let Kind::Split { children, .. } = &mut self.nodes.get_mut(&px).unwrap().kind {
            children[ix] = b;
        }
        if let Kind::Split { children, .. } = &mut self.nodes.get_mut(&py).unwrap().kind {
            children[iy] = a;
        }
        let a = self.nodes.get_mut(&a).unwrap();
        a.parent = Some(py);
        a.weight = y.weight;
        let b = self.nodes.get_mut(&b).unwrap();
        b.parent = Some(px);
        b.weight = x.weight;
        true
    }
    /// Remove a leaf or subtree, collapsing single-child splits. Returns its leaves.
    pub fn remove(&mut self, id: Id) -> Vec<Id> {
        let Some(ti) = self.tab_of(id) else {
            return Vec::new();
        };
        let removed = self.leaves(id);
        let Some(parent) = self.nodes[&id].parent else {
            self.free_subtree(id);
            self.tabs.remove(ti);
            if self.current > ti {
                self.current -= 1;
            } else if self.current >= self.tabs.len() {
                self.current = self.tabs.len().saturating_sub(1);
            }
            return removed;
        };
        let focus_lost = removed.contains(&self.tabs[ti].focus);
        if let Kind::Split { children, .. } = &mut self.nodes.get_mut(&parent).unwrap().kind {
            children.retain(|child| *child != id);
        }
        self.free_subtree(id);
        let children = self.nodes[&parent].children().to_vec();
        if children.is_empty() {
            let mut rest = self.remove(parent);
            let mut all = removed;
            all.append(&mut rest);
            return all;
        }
        let survivor = children[0];
        if children.len() == 1 {
            let grand = self.nodes[&parent].parent;
            self.nodes.get_mut(&survivor).unwrap().parent = grand;
            if let Some(grand) = grand {
                self.replace_child(grand, parent, survivor);
            } else {
                self.tabs[ti].root = survivor;
            }
            self.nodes.remove(&parent);
        }
        if focus_lost {
            self.tabs[ti].focus = self.first_leaf(survivor);
        }
        if self.tabs[ti]
            .zoom
            .is_some_and(|zoom| removed.contains(&zoom))
        {
            self.tabs[ti].zoom = None;
        }
        removed
    }
    fn free_subtree(&mut self, id: Id) {
        if let Some(node) = self.nodes.remove(&id) {
            for child in node.children() {
                self.free_subtree(*child);
            }
        }
    }
    fn live(&self, id: Id) -> bool {
        let node = &self.nodes[&id];
        if node.kind == Kind::Leaf {
            !node.minimized && !node.floating
        } else {
            node.children().iter().any(|id| self.live(*id))
        }
    }
    pub fn split_fits(&self, id: Id, direction: Direction, config: &Config) -> bool {
        let Some(node) = self.nodes.get(&id) else {
            return false;
        };
        if node.kind != Kind::Leaf || node.floating {
            return false;
        }
        let (minw, minh) = size(config, "min_pane");
        let (splitw, splith) = size(config, "min_split");
        let floor = match direction {
            Direction::Cols => minw.max(splitw),
            Direction::Rows => minh.max(splith),
        };
        let (k, r) = if let Some(parent) = node
            .parent
            .filter(|p| self.nodes[p].direction() == Some(direction))
        {
            (
                self.nodes[&parent].children().len() + 1,
                self.nodes[&parent].rect,
            )
        } else {
            (2, node.rect)
        };
        let span = match direction {
            Direction::Cols => r.w,
            Direction::Rows => r.h,
        } as u32;
        let across = match direction {
            Direction::Cols => node.rect.h >= minh,
            Direction::Rows => node.rect.w >= minw,
        };
        across && span >= k as u32 * floor as u32 + (k - 1) as u32 * gap(config, direction) as u32
    }
    pub fn sidebar_width(&self, cols: u16, config: &Config) -> u16 {
        let w = (config.number("tab_bar_width") as u16).clamp(8, 60);
        let spare = cols
            .saturating_sub(size(config, "min_pane").0.saturating_add(4))
            .min(cols / 3);
        if spare < w.min(14) { 0 } else { w.min(spare) }
    }
    pub fn sidebar<'a>(&self, cols: u16, rows: u16, config: &'a Config) -> &'a str {
        let side = config.text("tab_bar_side").unwrap_or("top");
        if side == "top" || !config.boolean("status_bar") || self.sidebar_width(cols, config) == 0 {
            return "top";
        }
        let need = if config.boolean("compact") {
            0
        } else {
            config.number("gap") as usize
        } + usize::from(config.boolean("tab_bar_chrome")) * 2
            + usize::from(
                config.boolean("newtab_button") && config.text_bytes("newtab_mark") != Some(b""),
            )
            + usize::from(config.boolean("status_line"))
            + self.tabs.len();
        if (rows as usize) < need && self.tabs.len() * 8 <= cols as usize {
            "top"
        } else {
            side
        }
    }
    pub fn area(&self, cols: u16, rows: u16, config: &Config) -> Rect {
        let compact = config.boolean("compact");
        let gx = if compact {
            u16::from(cols >= 3)
        } else {
            (config.number("gap") * config.number("gap_aspect")) as u16
        };
        let gy = if compact {
            u16::from(rows >= 3)
        } else {
            config.number("gap") as u16
        };
        let side = self.sidebar(cols, rows, config);
        let sw = if side != "top" {
            self.sidebar_width(cols, config)
        } else {
            0
        };
        let top = gy.saturating_add(u16::from(side == "top" && config.boolean("status_bar")));
        let wsub = gx.saturating_mul(2).saturating_add(sw);
        let hsub = top
            .saturating_add(gy)
            .saturating_add(u16::from(config.boolean("status_line")));
        Rect {
            x: gx.saturating_add(if side == "left" { sw } else { 0 }),
            y: top,
            w: if cols > wsub { cols - wsub } else { cols },
            h: if rows > hsub { rows - hsub } else { 1 },
        }
    }
    /// Probe before applying geometry, so hidden panes keep their last PTY size.
    pub fn arrange(&mut self, ti: usize, cols: u16, rows: u16, config: &Config) -> Vec<(Id, Rect)> {
        if ti >= self.tabs.len() {
            return Vec::new();
        }
        let area = self.area(cols, rows, config);
        let root = self.tabs[ti].root;
        let focus = self.tabs[ti].focus;
        let leaves = self.leaves(root);
        let mut resized = Vec::new();
        if !leaves.iter().any(|id| self.live(*id))
            && let Some(top) = leaves
                .iter()
                .filter(|id| self.nodes[id].floating && !self.nodes[id].minimized)
                .max_by_key(|id| self.nodes[id].raised)
                .copied()
        {
            self.nodes.get_mut(&top).unwrap().floating = false;
        }
        if let Some(node) = self.nodes.get_mut(&focus) {
            node.minimized = false;
            if node.floating && node.raised != self.raise_seq {
                self.raise_seq += 1;
                node.raised = self.raise_seq;
            }
        }
        self.tabs[ti].flattened = false;
        if let Some(zoom) = self.tabs[ti].zoom {
            if self.tab_of(zoom) == Some(ti) {
                self.flatten(root, area, zoom, false, config, &mut resized);
                return resized;
            }
            self.tabs[ti].zoom = None;
        }
        let minimized: Vec<_> = leaves
            .iter()
            .copied()
            .filter(|id| self.nodes[id].minimized)
            .collect();
        let mut tree = area;
        let mut no_room = false;
        self.tabs[ti].min_bar = Rect::default();
        if !minimized.is_empty() {
            let bar = if config.boolean("compact") { 4 } else { 3 };
            if area.h as u32 >= size(config, "min_pane").1 as u32 + 2 + bar {
                tree.h -= bar as u16;
                self.tabs[ti].min_bar = Rect {
                    x: area.x,
                    y: area.y + area.h - 3,
                    w: area.w,
                    h: 3,
                };
            } else {
                no_room = true;
            }
        }
        let overflow = self.place(root, tree, false, false, config, &mut Vec::new()) || no_room;
        if !overflow {
            self.place(root, tree, true, false, config, &mut resized);
            for id in minimized {
                self.collapse(
                    id,
                    Rect {
                        x: area.x,
                        y: area.y,
                        ..Rect::default()
                    },
                );
            }
            for id in leaves
                .iter()
                .copied()
                .filter(|id| self.nodes[id].floating && !self.nodes[id].minimized)
                .collect::<Vec<_>>()
            {
                let r = clamp_float(self.nodes[&id].float_rect, area, size(config, "min_pane"));
                self.place(id, r, true, true, config, &mut resized);
            }
        } else {
            self.tabs[ti].flattened = true;
            self.tabs[ti].min_bar = Rect::default();
            self.flatten(
                root,
                area,
                focus,
                area.h as usize >= leaves.len() + 2,
                config,
                &mut resized,
            );
        }
        resized
    }
    fn collapse(&mut self, id: Id, rect: Rect) {
        let node = self.nodes.get_mut(&id).unwrap();
        node.rect = rect;
        node.hidden = true;
        node.collapsed = true;
    }
    fn span_splits(&mut self, id: Id, rect: Rect) {
        let children = self.nodes[&id].children().to_vec();
        if children.is_empty() {
            return;
        }
        let node = self.nodes.get_mut(&id).unwrap();
        node.rect = rect;
        node.collapsed = false;
        for child in children {
            self.span_splits(child, rect);
        }
    }
    fn flatten(
        &mut self,
        root: Id,
        rect: Rect,
        focus: Id,
        stack: bool,
        config: &Config,
        resized: &mut Vec<(Id, Rect)>,
    ) {
        let leaves = self.leaves(root);
        if leaves.is_empty() {
            return;
        }
        let focus = if leaves.contains(&focus) {
            focus
        } else {
            leaves[0]
        };
        self.span_splits(root, rect);
        let mut y = rect.y;
        for id in leaves.iter().copied().filter(|id| *id != focus) {
            self.collapse(
                id,
                if stack {
                    Rect {
                        x: rect.x,
                        y,
                        w: rect.w,
                        h: 1,
                    }
                } else {
                    Rect {
                        x: rect.x,
                        y: rect.y,
                        ..Rect::default()
                    }
                },
            );
            if stack {
                y = y.saturating_add(1);
            }
        }
        self.place(
            focus,
            Rect {
                y,
                h: rect.h.saturating_sub(y - rect.y).max(1),
                ..rect
            },
            true,
            true,
            config,
            resized,
        );
    }
    fn place(
        &mut self,
        id: Id,
        rect: Rect,
        apply: bool,
        hidden: bool,
        config: &Config,
        resized: &mut Vec<(Id, Rect)>,
    ) -> bool {
        let node = &self.nodes[&id];
        if node.kind == Kind::Leaf && (node.minimized || node.floating) && !hidden {
            return false;
        }
        let kind = node.kind.clone();
        let floating = node.floating;
        let node = self.nodes.get_mut(&id).unwrap();
        node.rect = rect;
        node.collapsed = false;
        let (minw, minh) = size(config, "min_pane");
        let Kind::Split {
            direction,
            children,
        } = kind
        else {
            node.hidden = false;
            let classic = !config.boolean("compact") || floating || hidden;
            let framed = classic && rect.w >= 3 && rect.h >= 3;
            let border = u16::from(framed);
            let pads = framed || !classic;
            let pad = if pads {
                match config.setting("padding") {
                    Some(Setting::Sides(v)) => *v,
                    _ => [0; 4],
                }
            } else {
                [0; 4]
            };
            let aspect = config.number("gap_aspect") as u16;
            let left = border.saturating_add(pad[3].saturating_mul(aspect));
            let right = border.saturating_add(pad[1].saturating_mul(aspect));
            let top = border.saturating_add(pad[0]);
            let bottom = border.saturating_add(pad[2]);
            let content = Rect {
                x: rect.x.saturating_add(left),
                y: rect.y.saturating_add(top),
                w: rect.w.saturating_sub(left.saturating_add(right)).max(1),
                h: rect.h.saturating_sub(top.saturating_add(bottom)).max(1),
            };
            node.content = content;
            if apply {
                resized.push((id, content));
            }
            return rect.w < minw || rect.h < minh;
        };
        let live: Vec<_> = children.into_iter().filter(|id| self.live(*id)).collect();
        if live.is_empty() {
            return false;
        }
        if live.len() == 1 {
            return self.place(live[0], rect, apply, hidden, config, resized);
        }
        let gap = gap(config, direction) as u32;
        let total = match direction {
            Direction::Cols => rect.w,
            Direction::Rows => rect.h,
        } as u32;
        let floor = match direction {
            Direction::Cols => minw,
            Direction::Rows => minh,
        } as u32;
        let k = live.len() as u32;
        if total < k * floor + gap * (k - 1) {
            return true;
        }
        let avail = total.saturating_sub(gap * (k - 1));
        if avail < k {
            return true;
        }
        let total_weight = live
            .iter()
            .map(|id| self.nodes[id].weight)
            .sum::<i64>()
            .max(1);
        let mut sizes: Vec<u32> = live
            .iter()
            .map(|id| (avail as i64 * self.nodes[id].weight / total_weight).max(1) as u32)
            .collect();
        let mut used = sizes.iter().sum::<u32>();
        let mut i = 0;
        while used != avail {
            if used < avail {
                sizes[i] += 1;
                used += 1;
            } else if sizes[i] > 1 {
                sizes[i] -= 1;
                used -= 1;
            }
            i = (i + 1) % sizes.len();
        }
        let mut pos = match direction {
            Direction::Cols => rect.x,
            Direction::Rows => rect.y,
        } as u32;
        let mut overflow = false;
        for (id, span) in live.into_iter().zip(sizes) {
            let r = match direction {
                Direction::Cols => Rect {
                    x: pos as u16,
                    w: span as u16,
                    ..rect
                },
                Direction::Rows => Rect {
                    y: pos as u16,
                    h: span as u16,
                    ..rect
                },
            };
            overflow |= self.place(id, r, apply, hidden, config, resized);
            pos += span + gap;
        }
        overflow
    }
    pub fn focus_direction(&mut self, dx: i32, dy: i32) {
        let Some(focus) = self.focus() else {
            return;
        };
        let r = self.nodes[&focus].rect;
        let fx = r.x as i64 * 2 + r.w as i64;
        let fy = r.y as i64 * 2 + r.h as i64;
        let mut best = None;
        for id in self.leaves(self.tabs[self.current].root) {
            if id == focus {
                continue;
            }
            let r = self.nodes[&id].rect;
            let nx = r.x as i64 * 2 + r.w as i64;
            let ny = r.y as i64 * 2 + r.h as i64;
            let along = if dx != 0 {
                (nx - fx) * dx as i64
            } else {
                (ny - fy) * dy as i64
            };
            let across = if dx != 0 {
                (ny - fy).abs()
            } else {
                (nx - fx).abs()
            };
            if along > 0 && best.is_none_or(|(_, score)| along + across * 4 < score) {
                best = Some((id, along + across * 4));
            }
        }
        if let Some((id, _)) = best {
            self.tabs[self.current].focus = id;
        }
    }
    pub fn focus_next(&mut self) {
        let Some(focus) = self.focus() else {
            return;
        };
        let leaves = self.leaves(self.tabs[self.current].root);
        let i = leaves.iter().position(|id| *id == focus).unwrap_or(0);
        self.tabs[self.current].focus = leaves[(i + 1) % leaves.len()];
    }
    pub fn minimize(&mut self, id: Id) -> bool {
        let Some(ti) = self.tab_of(id) else {
            return false;
        };
        if self.nodes[&id].minimized {
            return false;
        }
        let Some(next) = self
            .leaves(self.tabs[ti].root)
            .into_iter()
            .find(|other| *other != id && self.live(*other))
        else {
            return false;
        };
        self.nodes.get_mut(&id).unwrap().minimized = true;
        self.current = ti;
        if self.tabs[ti].zoom == Some(id) {
            self.tabs[ti].zoom = None;
        }
        if self.tabs[ti].focus == id {
            self.tabs[ti].focus = next;
        }
        true
    }
    pub fn zoom(&mut self, id: Id) -> bool {
        let Some(ti) = self.tab_of(id) else {
            return false;
        };
        if self.nodes[&id].kind != Kind::Leaf {
            return false;
        }
        self.current = ti;
        self.tabs[ti].zoom = if self.tabs[ti].zoom == Some(id) {
            None
        } else {
            Some(id)
        };
        self.tabs[ti].focus = id;
        true
    }
    pub fn toggle_float(&mut self, id: Id, cols: u16, rows: u16, config: &Config) -> bool {
        let Some(ti) = self.tab_of(id) else {
            return false;
        };
        if self.nodes[&id].kind != Kind::Leaf {
            return false;
        }
        if self.nodes[&id].floating {
            self.nodes.get_mut(&id).unwrap().floating = false;
        } else {
            if !self
                .leaves(self.tabs[ti].root)
                .into_iter()
                .any(|other| other != id && self.live(other))
            {
                return false;
            }
            let area = self.area(cols, rows, config);
            self.raise_seq += 1;
            let n = self.nodes.get_mut(&id).unwrap();
            n.minimized = false;
            n.floating = true;
            n.raised = self.raise_seq;
            let mut w = n.float_rect.w;
            let mut h = n.float_rect.h;
            if w == 0 && n.rect.w > 4 && n.rect.h > 4 {
                w = n.rect.w - 2;
                h = n.rect.h - 2;
            }
            if w != 0 {
                w = w.min(area.w);
                h = h.min(area.h);
                n.float_rect = Rect {
                    x: area.x + (area.w - w) / 2,
                    y: area.y + (area.h - h) / 2,
                    w,
                    h,
                };
            }
        }
        self.current = ti;
        self.tabs[ti].focus = id;
        true
    }
    pub fn resize_focus(&mut self, dx: i32, dy: i32, config: &Config) {
        let Some(mut id) = self.focus() else {
            return;
        };
        let direction = if dx != 0 {
            Direction::Cols
        } else {
            Direction::Rows
        };
        loop {
            let Some(parent) = self.nodes[&id].parent else {
                return;
            };
            if self.nodes[&parent].direction() == Some(direction)
                && self.nodes[&parent].children().len() >= 2
            {
                break;
            }
            id = parent;
        }
        let parent = self.nodes[&id].parent.unwrap();
        let children = self.nodes[&parent].children();
        let i = children.iter().position(|child| *child == id).unwrap();
        let positive = if dx != 0 { dx > 0 } else { dy > 0 };
        let pair = if i + 1 < children.len() {
            if positive {
                (children[i + 1], id)
            } else {
                (id, children[i + 1])
            }
        } else if i > 0 {
            if positive {
                (id, children[i - 1])
            } else {
                (children[i - 1], id)
            }
        } else {
            return;
        };
        self.transfer(pair.0, pair.1, 120, config);
    }
    pub fn transfer(&mut self, from: Id, to: Id, amount: i64, config: &Config) {
        let mut minimum = 150;
        if let Some(parent) = self.nodes[&from].parent {
            let p = &self.nodes[&parent];
            let dir = p.direction().unwrap();
            let children = p.children();
            let floor = match dir {
                Direction::Cols => size(config, "min_pane").0,
                Direction::Rows => size(config, "min_pane").1,
            } as i64;
            let span = match dir {
                Direction::Cols => p.rect.w,
                Direction::Rows => p.rect.h,
            } as i64;
            let avail =
                (span - gap(config, dir) as i64 * (children.len().saturating_sub(1)) as i64).max(1);
            let total = children.iter().map(|id| self.nodes[id].weight).sum::<i64>();
            minimum = minimum.max((floor * total + avail - 1) / avail);
        }
        let amount = amount.min(self.nodes[&from].weight - minimum);
        if amount <= 0 {
            return;
        }
        self.nodes.get_mut(&from).unwrap().weight -= amount;
        self.nodes.get_mut(&to).unwrap().weight += amount;
    }
    pub fn equalize(&mut self) -> bool {
        let Some(tab) = self.tabs.get(self.current) else {
            return false;
        };
        let root = tab.root;
        if self.nodes[&root].kind == Kind::Leaf {
            return false;
        }
        self.equalize_node(root);
        true
    }
    pub fn boundary_for_side(&self, mut id: Id, side: char) -> Option<(Id, usize)> {
        let direction = if matches!(side, 'l' | 'r') {
            Direction::Cols
        } else {
            Direction::Rows
        };
        let before = matches!(side, 'l' | 't');
        while let Some(parent) = self.nodes.get(&id)?.parent {
            let p = &self.nodes[&parent];
            if p.direction() == Some(direction) {
                let i = p.children().iter().position(|child| *child == id)?;
                if before && i > 0 {
                    return Some((parent, i - 1));
                }
                if !before && i + 1 < p.children().len() {
                    return Some((parent, i));
                }
            }
            id = parent;
        }
        None
    }
    pub fn drag_edge(&mut self, split: Id, index: usize, dx: i32, dy: i32, config: &Config) {
        let Some(node) = self.nodes.get(&split) else {
            return;
        };
        let children = node.children();
        if index + 1 >= children.len() {
            return;
        }
        let (span, cells) = if node.direction() == Some(Direction::Cols) {
            (node.rect.w, dx)
        } else {
            (node.rect.h, dy)
        };
        if span == 0 || cells == 0 {
            return;
        }
        let total: i64 = children.iter().map(|id| self.nodes[id].weight).sum();
        let amount = (i64::from(cells).abs().saturating_mul(total) / i64::from(span)).max(1);
        let (a, b) = (children[index], children[index + 1]);
        self.transfer(
            if cells > 0 { b } else { a },
            if cells > 0 { a } else { b },
            amount,
            config,
        );
    }
    fn equalize_node(&mut self, id: Id) -> usize {
        let children = self.nodes[&id].children().to_vec();
        let n = if children.is_empty() {
            usize::from(self.live(id))
        } else {
            children.into_iter().map(|id| self.equalize_node(id)).sum()
        };
        self.nodes.get_mut(&id).unwrap().weight = WEIGHT_UNIT * n.max(1) as i64;
        n
    }
    pub fn rotate(&mut self, cols: u16, rows: u16, config: &Config) -> bool {
        let Some(tab) = self.tabs.get(self.current) else {
            return false;
        };
        let root = tab.root;
        let flat = tab.flattened;
        if self.nodes[&root].kind == Kind::Leaf {
            return false;
        }
        self.rotate_node(root);
        self.arrange(self.current, cols, rows, config);
        if self.tabs[self.current].flattened && !flat {
            for _ in 0..3 {
                self.rotate_node(root);
            }
            self.arrange(self.current, cols, rows, config);
            false
        } else {
            true
        }
    }
    fn rotate_node(&mut self, id: Id) {
        for child in self.nodes[&id].children().to_vec() {
            self.rotate_node(child);
        }
        if let Kind::Split {
            direction,
            children,
        } = &mut self.nodes.get_mut(&id).unwrap().kind
        {
            if *direction == Direction::Rows {
                children.reverse();
                *direction = Direction::Cols;
            } else {
                *direction = Direction::Rows;
            }
        }
    }
}
pub fn size(config: &Config, name: &str) -> (u16, u16) {
    match config.setting(name) {
        Some(Setting::Size { cols, rows }) => (*cols, *rows),
        _ => panic!("not a size: {name}"),
    }
}
pub fn gap(config: &Config, direction: Direction) -> u16 {
    if config.boolean("compact") {
        1
    } else {
        (config.number("gap")
            * if direction == Direction::Cols {
                config.number("gap_aspect")
            } else {
                1
            }) as u16
    }
}
fn clamp_float(mut r: Rect, area: Rect, floor: (u16, u16)) -> Rect {
    if r.w == 0 || r.h == 0 {
        r.w = (area.w as u32 * 2 / 3) as u16;
        r.h = (area.h as u32 * 2 / 3) as u16;
        r.x = area.x + (area.w - r.w) / 2;
        r.y = area.y + (area.h - r.h) / 2;
    }
    r.w = r.w.max(floor.0).min(area.w);
    r.h = r.h.max(floor.1).min(area.h);
    if r.x == u16::MAX {
        r.x = area.x + (area.w - r.w) / 2;
    }
    if r.y == u16::MAX {
        r.y = area.y + (area.h - r.h) / 2;
    }
    r.x = r.x.max(area.x).min(area.x + area.w - r.w);
    r.y = r.y.max(area.y).min(area.y + area.h - r.h);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_moves_preserve_leaf_identity_and_a_connected_tree() {
        let mut layout = Layout::default();
        let mut leaves = Vec::new();
        for _ in 0..3 {
            let first = layout.leaf();
            layout.add_tab(first, "");
            leaves.push(first);
            let second = layout.leaf();
            layout.insert_beside(first, second, Direction::Cols, false);
            leaves.push(second);
        }
        for step in 0..300 {
            let id = leaves[step % leaves.len()];
            let from = layout.tab_of(id).unwrap();
            let dest = (from + 1) % layout.tabs.len();
            let destination = layout.tabs[dest].id;
            let focus = layout.tabs[dest].focus;
            let take_focus = step % 2 == 0;
            assert!(layout.move_leaf(
                id,
                destination,
                0,
                if step % 3 == 0 {
                    Direction::Rows
                } else {
                    Direction::Cols
                },
                take_focus
            ));
            assert_eq!(
                layout
                    .tabs
                    .iter()
                    .find(|t| t.id == destination)
                    .unwrap()
                    .focus,
                if take_focus { id } else { focus }
            );
            let mut found = Vec::new();
            for tab in &layout.tabs {
                assert!(layout.nodes[&tab.root].parent.is_none());
                let panes = layout.leaves(tab.root);
                assert!(panes.contains(&tab.focus));
                found.extend(panes);
            }
            found.sort();
            let mut want = leaves.clone();
            want.sort();
            assert_eq!(found, want);
            for node in layout.nodes.values() {
                if let Some(parent) = node.parent {
                    assert!(layout.nodes[&parent].children().contains(&node.id));
                }
                if node.kind != Kind::Leaf {
                    assert!(node.children().len() >= 2);
                }
            }
        }
    }
}
