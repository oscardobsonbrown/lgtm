use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

pub(crate) struct PreviewSession {
    root: PathBuf,
}

impl PreviewSession {
    pub(crate) fn new() -> Self {
        let parent = preview_parent_root();
        cleanup_stale_sessions(&parent, std::process::id(), process_is_alive);
        Self {
            root: parent.join(std::process::id().to_string()),
        }
    }

    pub(crate) fn item(&self, item_id: u64) -> PreviewItem {
        PreviewItem {
            root: self.root.join(item_id.to_string()),
            next_generation: 0,
            active: None,
        }
    }
}

impl Drop for PreviewSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) struct PreviewItem {
    root: PathBuf,
    next_generation: u64,
    active: Option<PreviewGeneration>,
}

impl PreviewItem {
    pub(crate) fn begin(&mut self) -> PreviewGeneration {
        self.next_generation += 1;
        PreviewGeneration(Arc::new(Generation {
            root: self.root.join(self.next_generation.to_string()),
        }))
    }

    pub(crate) fn activate(&mut self, generation: PreviewGeneration) {
        self.active = Some(generation);
    }

    pub(crate) fn active(&self) -> Option<PreviewGeneration> {
        self.active.clone()
    }
}

impl Drop for PreviewItem {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone)]
pub(crate) struct PreviewGeneration(Arc<Generation>);

impl PreviewGeneration {
    pub(crate) fn repo_path(&self) -> PathBuf {
        self.0.root.join("repo.git")
    }
    pub(crate) fn blob_path(&self) -> PathBuf {
        self.0.root.join("blobs")
    }
    #[cfg(test)]
    fn root(&self) -> &Path {
        &self.0.root
    }
}

struct Generation {
    root: PathBuf,
}

impl Drop for Generation {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn preview_parent_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache/lgtm/previews")
}

fn process_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn cleanup_stale_sessions(parent: &Path, current_pid: u32, mut alive: impl FnMut(u32) -> bool) {
    let _ = std::fs::create_dir_all(parent);
    let _ = std::fs::remove_dir_all(parent.join(current_pid.to_string()));
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        if pid != current_pid && !alive(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("lgtm-preview-{name}-{}", std::process::id()))
    }

    #[test]
    fn generations_are_isolated_and_old_work_cleans_last() {
        let base = root("generations");
        let mut item = PreviewItem {
            root: base.clone(),
            next_generation: 0,
            active: None,
        };
        let first = item.begin();
        let worker = first.clone();
        std::fs::create_dir_all(first.blob_path()).unwrap();
        item.activate(first);
        let second = item.begin();
        std::fs::create_dir_all(second.blob_path()).unwrap();
        item.activate(second);
        assert!(worker.root().exists());
        drop(worker);
        assert!(!base.join("1").exists());
        assert!(base.join("2").exists());
        drop(item);
        assert!(!base.exists());
    }

    #[test]
    fn cleanup_removes_only_current_and_dead_sessions() {
        let base = root("sessions");
        for name in ["10", "20", "30", "not-a-pid"] {
            std::fs::create_dir_all(base.join(name)).unwrap();
        }
        cleanup_stale_sessions(&base, 10, |pid| pid == 20);
        assert!(!base.join("10").exists());
        assert!(base.join("20").exists());
        assert!(!base.join("30").exists());
        assert!(base.join("not-a-pid").exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
