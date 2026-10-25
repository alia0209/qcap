//! Log files: /tmp/q/<slug>-<n>.log, slug from the first two words, n one past the highest taken.

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub fn dir() -> PathBuf {
    std::env::var_os("Q_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/q"))
}

/// `["bun", "run", "validate"]` -> `bun-run`; `["~/x/validate-quiet"]` -> `validate-quiet`;
/// `["bash", "-c", "seq 3"]` -> `bash-seq`. Shell strings are split on whitespace; flags and
/// `VAR=value` words are skipped.
pub fn slug(cmd: &[String]) -> String {
    let parts: Vec<String> = cmd
        .iter()
        .flat_map(|a| a.split_whitespace())
        .filter(|w| !w.starts_with('-') && !w.contains('='))
        .take(2)
        .map(|w| clean(w.rsplit('/').next().unwrap_or(w)))
        .filter(|w| !w.is_empty())
        .collect();
    if parts.is_empty() {
        "cmd".into()
    } else {
        parts.join("-")
    }
}

fn clean(word: &str) -> String {
    let s: String = word
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches(|c| c == '-' || c == '.').to_string();
    s.chars().take(24).collect()
}

/// Logs older than this are deleted when the next one is created.
const KEEP: Duration = Duration::from_secs(2 * 24 * 3600);

/// Creates the next free `<slug>-<n>.log` atomically (create_new), so parallel runs never share a
/// file, and records it as the newest log for the current directory.
pub fn create(dir: &Path, slug: &str) -> io::Result<(PathBuf, File)> {
    fs::create_dir_all(dir)?;
    prune(dir);
    let mut n = highest(dir, slug) + 1;
    loop {
        let path = dir.join(format!("{slug}-{n}.log"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => {
                if let Some(marker) = cwd_marker(dir) {
                    let _ = fs::create_dir_all(dir.join("by-cwd"));
                    let _ = fs::write(marker, path.display().to_string());
                }
                return Ok((path, f));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    }
}

fn highest(dir: &Path, slug: &str) -> u64 {
    let prefix = format!("{slug}-");
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_prefix(&prefix)?
                .strip_suffix(".log")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

fn prune(dir: &Path) {
    let now = SystemTime::now();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > KEEP));
        if old && e.path().extension().is_some_and(|x| x == "log") {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// `<dir>/by-cwd/<hash of $PWD>`: holds the path of the newest log started from that directory.
fn cwd_marker(dir: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut h = DefaultHasher::new();
    cwd.hash(&mut h);
    Some(dir.join("by-cwd").join(format!("{:016x}", h.finish())))
}

/// Newest log started from the current directory, else the newest log overall. Parallel agents in
/// other directories don't steal `q last`; agents sharing a directory should pass the log path.
pub fn newest(dir: &Path) -> Option<PathBuf> {
    cwd_marker(dir)
        .and_then(|m| fs::read_to_string(m).ok())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| newest_any(dir))
}

fn newest_any(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max()
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn slugs() {
        assert_eq!(slug(&s(&["bun", "run", "validate"])), "bun-run");
        assert_eq!(slug(&s(&["bun test --bail"])), "bun-test");
        assert_eq!(slug(&s(&["/usr/local/bin/validate-all"])), "validate-all");
        assert_eq!(slug(&s(&["cargo", "--version"])), "cargo");
        assert_eq!(slug(&s(&["bash", "-c", "seq 3; exit 1"])), "bash-seq");
        assert_eq!(slug(&s(&["FOO=1 bun test"])), "bun-test");
        assert_eq!(slug(&s(&["echo", "$X|y"])), "echo-x-y");
        assert_eq!(slug(&s(&["|"])), "cmd");
    }

    #[test]
    fn numbering_increments() {
        let d = std::env::temp_dir().join(format!("q-logpath-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (a, _) = create(&d, "x-y").unwrap();
        let (b, _) = create(&d, "x-y").unwrap();
        assert!(a.ends_with("x-y-1.log") && b.ends_with("x-y-2.log"));
        assert!(newest(&d).is_some());
        fs::remove_dir_all(&d).unwrap();
    }
}
