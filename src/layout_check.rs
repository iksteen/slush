//! Non-destructive validation of the session document schema.
use crate::{config::Config, kdl, path};

pub const PANE_PROPS: &[&str] = &[
    "split",
    "weight",
    "cwd",
    "command",
    "focus",
    "purpose",
    "suspended",
    "floating",
    "x",
    "y",
    "w",
    "h",
];
pub const TAB_PROPS: &[&str] = &["name", "active"];

#[derive(Debug, Default)]
pub struct Report {
    pub messages: Vec<String>,
    pub dropped: usize,
}

struct Checker<'a> {
    file: &'a str,
    report: Report,
}
impl Checker<'_> {
    fn say(&mut self, line: usize, text: impl AsRef<str>) {
        if self.report.messages.len() == 32 {
            self.report.dropped += 1;
            return;
        }
        // Match the diagnostic buffers, preserving complete UTF-8 characters.
        let truncate = |s: &str, cap: usize| {
            let mut end = s.len().min(cap - 1);
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s[..end].to_owned()
        };
        self.report.messages.push(truncate(
            &format!("{}:{line}: {}", self.file, truncate(text.as_ref(), 160)),
            192,
        ));
    }
    fn props(&mut self, node: &kdl::Node, is_tab: bool) {
        let kids = node.kids.iter().any(|n| n.name == "pane");
        let kind = if is_tab { "tab" } else { "pane" };
        for (key, _) in &node.props {
            let key = key.as_str();
            if !PANE_PROPS.contains(&key) && !(is_tab && TAB_PROPS.contains(&key)) {
                self.say(node.line, format!("unknown {kind} property: {key}"));
                continue;
            }
            let leaf_only = matches!(
                key,
                "command" | "suspended" | "focus" | "floating" | "x" | "y" | "w" | "h"
            ) || (!is_tab && key == "purpose");
            if kids && leaf_only {
                self.say(
                    node.line,
                    format!("{key} is ignored on a {kind} with panes in it"),
                );
            }
            if !kids && key == "split" {
                self.say(
                    node.line,
                    "split is ignored on a pane with nothing to split",
                );
            }
            if !kids && matches!(key, "x" | "y" | "w" | "h") && !node.prop_bool("floating", false) {
                self.say(node.line, format!("{key} is ignored without floating=true"));
            }
        }
        if let Some(split) = node.property("split")
            && !matches!(split, "cols" | "rows")
        {
            self.say(node.line, format!("split is cols or rows, not `{split}`"));
        }
        if let Some(weight) = node.property("weight") {
            // strtol accepts leading space and a sign, but rejects trailing bytes.
            let value = weight.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
            if digits.is_empty()
                || !digits.bytes().all(|b| b.is_ascii_digit())
                || !kdl::integer(value).is_some_and(|v| v >= 150)
            {
                self.say(
                    node.line,
                    format!("weight is a number of 150 or more, not `{weight}`"),
                );
            }
        }
        for key in ["suspended", "focus", "floating"]
            .into_iter()
            .chain(is_tab.then_some("active"))
        {
            if let Some(value) = node.property(key)
                && !matches!(value, "true" | "false")
            {
                self.say(
                    node.line,
                    format!("{key} takes true or false, not `{value}`"),
                );
            }
        }
        for child in &node.kids {
            if child.name == "pane" {
                self.props(child, false);
            } else {
                self.say(
                    child.line,
                    format!("a {kind} holds panes, not `{}`", child.name),
                );
            }
        }
    }
}

pub fn check(root: &kdl::Node, file: &str) -> Report {
    let mut checker = Checker {
        file,
        report: Report::default(),
    };
    let config = Config::default();
    let layout = root.child("layout");
    for node in &root.kids {
        if layout.is_some_and(|lay| std::ptr::eq(lay, node)) || node.name == "tab" {
            continue;
        }
        if config.is_setting(&node.name) {
            checker.say(
                node.line,
                format!(
                    "this is a config, not a layout: `{}` is a setting",
                    node.name
                ),
            );
        } else {
            checker.say(node.line, format!("unknown node: {}", node.name));
        }
    }
    let lay = layout.unwrap_or(root);
    let mut tabs = 0;
    for node in &lay.kids {
        if node.name == "tab" {
            tabs += 1;
            checker.props(node, true);
        } else if layout.is_some() {
            checker.say(
                node.line,
                format!("a layout holds tabs, not `{}`", node.name),
            );
        }
    }
    if tabs == 0 {
        checker.say(lay.line, "this layout declares no tabs");
    }
    checker.report
}

pub fn looks_like_layout(file: impl AsRef<std::path::Path>) -> bool {
    let file = file.as_ref();
    match std::fs::read(path::expand_os(file))
        .ok()
        .and_then(|s| kdl::parse_bytes(&s).ok())
    {
        Some(root) => root
            .kids
            .iter()
            .any(|n| matches!(n.name.as_str(), "layout" | "tab")),
        None => {
            let bytes = file.as_os_str().as_encoded_bytes();
            bytes.ends_with(b".layout") || bytes.windows(8).any(|s| s == b".layout.")
        }
    }
}

pub fn check_file(file: impl AsRef<std::path::Path>) -> Report {
    let file = path::expand_os(file.as_ref());
    let display = file.to_string_lossy();
    match std::fs::read(&file) {
        Ok(source) => match kdl::parse_bytes(&source) {
            Ok(root) => check(&root, path::base(&display)),
            Err(e) => Report {
                messages: vec![format!(
                    "{}:{}: {}",
                    path::base(&display),
                    e.line,
                    e.message
                )],
                dropped: 0,
            },
        },
        Err(_) => Report {
            messages: vec![format!("cannot open {}", file.display())],
            dropped: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_keep_source_order_and_count_overflow() {
        let mut source = "layout {\n tab active=yes split=rows {\n pane split=wrong weight=149 purpose=x { pane; wobble }\n".to_owned();
        for _ in 0..40 {
            source.push_str(" pane cmd=oops\n");
        }
        source.push_str(" }\n}\n");
        let report = check(&kdl::parse(&source).unwrap(), "session.layout");
        assert_eq!(report.messages.len(), 32);
        assert_eq!(report.dropped, 13);
        assert_eq!(
            report.messages[0],
            "session.layout:2: active takes true or false, not `yes`"
        );
        assert_eq!(
            report.messages[1],
            "session.layout:3: purpose is ignored on a pane with panes in it"
        );
        assert!(report.messages[4].ends_with("a pane holds panes, not `wobble`"));
    }
}
