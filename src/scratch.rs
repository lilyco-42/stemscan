//! A private scratch directory owned by exactly one invocation.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub struct Scratch {
    path: PathBuf,
    keep: bool,
}

impl Scratch {
    pub fn new(keep: bool) -> std::io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        for _ in 0..128 {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("stemscan-{}-{stamp}-{id}", std::process::id()));
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path, keep }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Could not allocate a unique stemscan scratch directory",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_invocations_cannot_remove_each_others_audio() {
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let scratch = Scratch::new(false).unwrap();
                    let file = scratch.path().join("input.wav");
                    let expected = format!("audio for invocation {index}");
                    std::fs::write(&file, &expected).unwrap();
                    barrier.wait();
                    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
                    scratch.path().to_path_buf()
                })
            })
            .collect();
        let paths: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert!(paths.iter().all(|path| !path.exists()));
        let distinct: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(distinct.len(), 8);
    }

    #[test]
    fn keeping_one_result_does_not_keep_another_invocations_scratch() {
        let kept = Scratch::new(true).unwrap();
        let disposable = Scratch::new(false).unwrap();
        let kept_path = kept.path().to_path_buf();
        let disposable_path = disposable.path().to_path_buf();
        std::fs::write(kept_path.join("Vocals.wav"), b"retained result").unwrap();
        drop(disposable);
        drop(kept);
        assert!(!disposable_path.exists());
        assert_eq!(
            std::fs::read(kept_path.join("Vocals.wav")).unwrap(),
            b"retained result"
        );
        std::fs::remove_dir_all(kept_path).unwrap();
    }
}
