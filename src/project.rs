//! On-demand discovery of project directories under configured roots.
use crate::{json_control::native_bytes, path};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

pub const LAYOUT_FILE: &str = "slosh.layout";
pub const OLD_LAYOUT_FILE: &str = "slosh.layout.kdl";
const MAX_PROJECTS: usize = 128;

#[derive(Clone, Debug)]
pub struct Project {
    pub name: Vec<u8>,
    pub path: PathBuf,
    pub slug: Vec<u8>,
    pub layout: Option<PathBuf>,
    pub mtime: i64,
}

pub fn slug(path: impl AsRef<Path>, name: impl AsRef<[u8]>) -> Vec<u8> {
    let hash = native_bytes(path.as_ref().as_os_str())
        .iter()
        .fold(2166136261u32, |h, b| (h ^ *b as u32).wrapping_mul(16777619));
    let name = name.as_ref();
    let mut out = b"project:".to_vec();
    out.extend_from_slice(&name[..name.len().min(46)]);
    out.extend_from_slice(format!(".{hash:08x}").as_bytes());
    out
}

fn describe(dir: &Path) -> Option<Project> {
    if !dir.is_dir() {
        return None;
    }
    let resolved = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    let bytes = native_bytes(resolved.file_name()?);
    let name = bytes[..bytes.len().min(63)].to_vec();
    if name.is_empty() || name.starts_with(b".") {
        return None;
    }
    let mut project = Project {
        slug: slug(&resolved, &name),
        name,
        path: resolved.clone(),
        layout: None,
        mtime: 0,
    };
    for file in [LAYOUT_FILE, OLD_LAYOUT_FILE] {
        let file = resolved.join(file);
        if let Ok(metadata) = fs::metadata(&file) {
            project.layout = Some(file);
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                project.mtime = metadata.mtime();
            }
            #[cfg(not(unix))]
            {
                project.mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
            }
            return Some(project);
        }
    }
    resolved.join(".git").exists().then_some(project)
}

fn scan_dir(dir: &Path, depth: u8, out: &mut Vec<Project>) {
    if depth == 0 || out.len() == MAX_PROJECTS {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() == MAX_PROJECTS {
            break;
        }
        if native_bytes(&entry.file_name()).starts_with(b".") {
            continue;
        }
        let dir = entry.path();
        if let Some(project) = describe(&dir) {
            if !out.iter().any(|p| p.path == project.path) {
                out.push(project);
            }
        } else if dir.is_dir() {
            scan_dir(&dir, depth - 1, out);
        }
    }
}

pub fn scan(roots: &[PathBuf], depth: u8) -> Vec<Project> {
    let mut projects = Vec::new();
    for root in roots.iter().take_while(|s| !s.as_os_str().is_empty()) {
        scan_dir(&path::expand_os(root), depth, &mut projects);
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    projects
}

pub fn find(roots: &[PathBuf], depth: u8, name: impl AsRef<OsStr>) -> Option<Project> {
    let name = name.as_ref();
    if name.is_empty() {
        return None;
    }
    let bytes = native_bytes(name);
    let wanted = if bytes.contains(&b'/') || (cfg!(windows) && bytes.contains(&b'\\')) {
        Some(fs::canonicalize(path::expand_os(Path::new(name))).ok()?)
    } else {
        None
    };
    scan(roots, depth).into_iter().find(|p| {
        wanted
            .as_ref()
            .map_or(p.name == bytes.as_ref(), |w| p.path == *w)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scan_respects_depth_deduplicates_links_and_stops_at_projects() {
        let base = std::env::temp_dir().join(format!(
            "slush-project-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for dir in [
            "api/.git",
            "api/vendor/inner/.git",
            "team/api/.git",
            ".hidden/.git",
        ] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(base.join("api"), base.join("alias")).unwrap();
        let roots = vec![base.clone(), base.join("team")];
        let projects = scan(&roots, 2);
        assert_eq!(projects.len(), 2);
        assert!(projects.iter().all(|p| p.name == b"api"));
        assert_ne!(projects[0].slug, projects[1].slug);
        assert_eq!(scan(&roots[..1], 1).len(), 1);
        assert!(find(&roots, 2, "/tmp").is_none());
        assert!(find(&roots, 2, &projects[0].path).is_some());
        assert_eq!(slug("/dev/api", "api"), b"project:api.117e308a");
        fs::remove_dir_all(base).unwrap();
    }
}
