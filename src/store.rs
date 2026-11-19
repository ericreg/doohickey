use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Task {
    pub text: String,
    pub done: bool,
}

pub struct Store {
    path: PathBuf,
    tasks: Vec<Task>,
    _lock: File,
}

impl Store {
    pub fn open(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let lock = File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("tasks.lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "doohickey is already running. Click its checkmark in the menu bar.",
                ));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
        let path = directory.join("tasks.json");
        let tasks = match fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", path.display()),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            tasks,
            _lock: lock,
        })
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn add(&mut self, text: &str) -> io::Result<bool> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(false);
        }
        let mut tasks = self.tasks.clone();
        tasks.push(Task {
            text: text.to_owned(),
            done: false,
        });
        self.commit(tasks)?;
        Ok(true)
    }

    pub fn set_done(&mut self, index: usize, done: bool) -> io::Result<()> {
        let mut tasks = self.tasks.clone();
        tasks
            .get_mut(index)
            .ok_or_else(|| io::Error::other("This task no longer exists."))?
            .done = done;
        self.commit(tasks)
    }

    pub fn rename(&mut self, index: usize, text: &str) -> io::Result<()> {
        let task = self
            .tasks
            .get(index)
            .ok_or_else(|| io::Error::other("This task no longer exists."))?;
        let text = text.trim();
        if text.is_empty() || text == task.text {
            return Ok(());
        }
        let mut tasks = self.tasks.clone();
        tasks[index].text = text.to_owned();
        self.commit(tasks)
    }

    pub fn remove(&mut self, index: usize) -> io::Result<()> {
        if index >= self.tasks.len() {
            return Err(io::Error::other("This task no longer exists."));
        }
        let mut tasks = self.tasks.clone();
        tasks.remove(index);
        self.commit(tasks)
    }

    fn commit(&mut self, tasks: Vec<Task>) -> io::Result<()> {
        let temporary = self.path.with_extension("tmp");
        fs::write(&temporary, serde_json::to_vec(&tasks)?)?;
        fs::rename(temporary, &self.path)?;
        self.tasks = tasks;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn checklist_survives_restarts_and_failed_writes() -> io::Result<()> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("doohickey-{}-{stamp}", std::process::id()));
        let mut store = Store::open(&dir)?;
        assert!(matches!(Store::open(&dir), Err(e) if e.kind() == io::ErrorKind::WouldBlock));
        assert!(!store.add("  \n ")?);
        assert!(store.add("  Buy café beans ☕  ")?);
        assert!(store.add("Read a chapter")?);
        store.set_done(0, true)?;
        drop(store);

        let mut store = Store::open(&dir)?;
        assert_eq!(store.tasks().len(), 2);
        assert_eq!(store.tasks()[0].text, "Buy café beans ☕");
        assert!(store.tasks()[0].done);
        assert!(!store.tasks()[1].done);
        fs::create_dir(dir.join("tasks.tmp"))?;
        assert!(store.set_done(0, false).is_err());
        assert!(store.remove(1).is_err());
        assert!(store.rename(0, "Replacement").is_err());
        assert_eq!(store.tasks()[0].text, "Buy café beans ☕");
        assert_eq!(store.tasks().len(), 2);
        assert!(store.tasks()[0].done);
        fs::remove_dir(dir.join("tasks.tmp"))?;
        store.rename(0, "  Pick up espresso ☕  ")?;
        store.rename(0, "   ")?;
        assert!(store.tasks()[0].done);
        store.set_done(0, false)?;
        store.remove(1)?;
        assert!(store.remove(1).is_err());
        drop(store);
        let store = Store::open(&dir)?;
        assert_eq!(store.tasks().len(), 1);
        assert_eq!(store.tasks()[0].text, "Pick up espresso ☕");
        assert!(!store.tasks()[0].done);
        drop(store);

        fs::write(dir.join("tasks.json"), b"broken")?;
        assert!(matches!(Store::open(&dir), Err(e) if e.kind() == io::ErrorKind::InvalidData));
        assert_eq!(fs::read(dir.join("tasks.json"))?, b"broken");
        fs::remove_dir_all(dir)
    }
}
