use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
};
const LIMIT: u64 = 5_000_000;
#[derive(Clone)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
    pub original: String,
    pub read_only: bool,
    hash: Vec<u8>,
}
impl Document {
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let meta = fs::metadata(&path)?;
        if !meta.is_file() || meta.len() > LIMIT {
            bail!("Open a regular file up to 5 MB");
        }
        let bytes = fs::read(&path)?;
        if bytes.contains(&0) {
            bail!("Binary file; use an external application");
        }
        let hash = Sha256::digest(&bytes).to_vec();
        // from_utf8 takes ownership; on failure the bytes come back without a copy.
        let (text, read_only) = match String::from_utf8(bytes) {
            Ok(t) => (t, false),
            Err(e) => (
                e.into_bytes().iter().map(|b| char::from(*b)).collect(),
                true,
            ),
        };
        Ok(Self {
            path,
            original: text.clone(),
            text,
            read_only,
            hash,
        })
    }
    pub fn dirty(&self) -> bool {
        self.text != self.original
    }
    pub fn save(&mut self) -> Result<()> {
        self.hash = write_checked(&self.path, &self.text, &self.hash, self.read_only)?;
        self.original = self.text.clone();
        Ok(())
    }
}
/// A save found the file on disk no longer matches the content the document was
/// opened (or last saved) with. Nothing was written.
#[derive(Debug)]
pub struct Conflict;
impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("File changed on disk. Reopen it before saving.")
    }
}
impl std::error::Error for Conflict {}
/// Writes `text` to `path` only if the file on disk still hashes to `expected`:
/// temp file in the same directory, fsync, atomic rename. Returns the new hash.
fn write_checked(path: &Path, text: &str, expected: &[u8], read_only: bool) -> Result<Vec<u8>> {
    if read_only {
        bail!("Unknown encoding: file is read-only");
    }
    if text.len() as u64 > LIMIT {
        bail!("File exceeds 5 MB");
    }
    let current = fs::read(path)?;
    if Sha256::digest(&current).as_slice() != expected {
        return Err(Conflict.into());
    }
    let meta = fs::metadata(path)?;
    if meta.permissions().readonly() {
        bail!("File is read-only");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            bail!("Saving a multiply linked file needs an external editor");
        }
    }
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().context("No parent directory")?)?;
    #[cfg(unix)]
    preserve_owner(temp.as_file(), path, &meta)?;
    temp.as_file().set_permissions(meta.permissions())?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(Sha256::digest(text.as_bytes()).to_vec())
}
/// The replace-by-rename save gives the new file the process's uid/gid. Hand the
/// temp file the original owner instead; if that is not permitted (e.g. editing
/// another user's file through group write access), refuse rather than silently
/// changing who owns the file.
#[cfg(unix)]
fn preserve_owner(temp: &fs::File, path: &Path, meta: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::AsRawFd;
    let current = temp.metadata()?;
    if current.uid() == meta.uid() && current.gid() == meta.gid() {
        return Ok(());
    }
    // SAFETY: fd is a valid open descriptor owned by `temp` for this call.
    let rc = unsafe { libc::fchown(temp.as_raw_fd(), meta.uid(), meta.gid()) };
    if rc != 0 {
        bail!(
            "Saving {} would change its owner (uid {}, gid {}) to you: {}. Save a copy elsewhere or use an external editor.",
            path.display(),
            meta.uid(),
            meta.gid(),
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}
/// Work handed to the document IO thread.
enum Job {
    Open {
        id: String,
        request: u64,
        path: PathBuf,
    },
    Save {
        id: String,
        request: u64,
        path: PathBuf,
        text: String,
        expected: Vec<u8>,
        read_only: bool,
    },
}
enum Done {
    Opened {
        id: String,
        request: u64,
        result: Result<Document>,
    },
    Saved {
        id: String,
        request: u64,
        path: PathBuf,
        text: String,
        result: Result<Vec<u8>>,
    },
}
fn run(job: Job) -> Done {
    match job {
        Job::Open { id, request, path } => Done::Opened {
            id,
            request,
            result: Document::open(&path),
        },
        Job::Save {
            id,
            request,
            path,
            text,
            expected,
            read_only,
        } => {
            let result = write_checked(&path, &text, &expected, read_only);
            Done::Saved {
                id,
                request,
                path,
                text,
                result,
            }
        }
    }
}
/// What [`DocumentIo::poll`] / [`DocumentIo::flush`] applied, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocEvent {
    Opened {
        id: String,
    },
    OpenFailed {
        id: String,
        error: String,
        /// The open came from "open file" and its pane should go away on failure.
        remove_pane: bool,
    },
    Saved {
        id: String,
        path: PathBuf,
    },
    SaveFailed {
        id: String,
        error: String,
        /// The file changed on disk; nothing was written.
        conflict: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRequest {
    /// A save of the current text was handed to the IO thread.
    Started,
    /// A save of this document is already running; the latest text is saved after it.
    Queued,
}
struct PendingOpen {
    request: u64,
    remove_pane: bool,
}
struct InFlightSave {
    request: u64,
    queued: bool,
}
/// Opens and saves editor documents on a background thread so file IO never
/// stalls a frame. Results come back over a channel tagged with request ids;
/// an open superseded by a newer one is dropped. At most one save per document
/// is in flight; further requests coalesce and save the latest text once the
/// running save completes (its new hash is the next save's conflict baseline).
pub struct DocumentIo {
    jobs: Option<mpsc::Sender<Job>>,
    done_tx: mpsc::Sender<Done>,
    done: mpsc::Receiver<Done>,
    thread: Option<std::thread::JoinHandle<()>>,
    next: u64,
    opens: HashMap<String, PendingOpen>,
    saves: HashMap<String, InFlightSave>,
    failed: HashMap<String, String>,
    saves_submitted: u64,
}
impl DocumentIo {
    /// `wake` runs on the IO thread after each result (e.g. request a repaint).
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done) = mpsc::channel();
        let results = done_tx.clone();
        let thread = std::thread::Builder::new()
            .name("caelum-docio".into())
            .spawn(move || {
                for job in job_rx {
                    if results.send(run(job)).is_err() {
                        return;
                    }
                    wake();
                }
            })
            .ok();
        Self {
            jobs: thread.as_ref().map(|_| jobs),
            done_tx,
            done,
            thread,
            next: 0,
            opens: HashMap::new(),
            saves: HashMap::new(),
            failed: HashMap::new(),
            saves_submitted: 0,
        }
    }
    fn submit(&mut self, job: Job) {
        let job = match &self.jobs {
            Some(tx) => match tx.send(job) {
                Ok(()) => return,
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        // No IO thread: do the work inline rather than lose it.
        let _ = self.done_tx.send(run(job));
    }
    fn request(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
    /// Starts loading `path` for document `id`; supersedes any open already running for it.
    pub fn open(&mut self, id: &str, path: &Path, remove_pane: bool) {
        let request = self.request();
        self.failed.remove(id);
        self.opens.insert(
            id.to_string(),
            PendingOpen {
                request,
                remove_pane,
            },
        );
        self.submit(Job::Open {
            id: id.to_string(),
            request,
            path: path.to_path_buf(),
        });
    }
    pub fn loading(&self, id: &str) -> bool {
        self.opens.contains_key(id)
    }
    pub fn open_error(&self, id: &str) -> Option<&str> {
        self.failed.get(id).map(String::as_str)
    }
    pub fn saving(&self, id: &str) -> bool {
        self.saves.contains_key(id)
    }
    /// Saves submitted to the IO thread so far (coalesced requests are not counted).
    pub fn saves_submitted(&self) -> u64 {
        self.saves_submitted
    }
    /// Snapshots `doc`'s text and conflict baseline and saves them in the background.
    pub fn save(&mut self, id: &str, doc: &Document) -> SaveRequest {
        if let Some(running) = self.saves.get_mut(id) {
            running.queued = true;
            return SaveRequest::Queued;
        }
        self.start_save(id, doc);
        SaveRequest::Started
    }
    fn start_save(&mut self, id: &str, doc: &Document) {
        let request = self.request();
        self.saves.insert(
            id.to_string(),
            InFlightSave {
                request,
                queued: false,
            },
        );
        self.saves_submitted += 1;
        self.submit(Job::Save {
            id: id.to_string(),
            request,
            path: doc.path.clone(),
            text: doc.text.clone(),
            expected: doc.hash.clone(),
            read_only: doc.read_only,
        });
    }
    /// Drops open state for documents whose panes are gone. Saves already running
    /// still complete (and [`Self::flush`] still waits for them).
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.opens.retain(|id, _| keep(id));
        self.failed.retain(|id, _| keep(id));
    }
    /// Applies every result that has arrived, without blocking.
    pub fn poll(&mut self, docs: &mut HashMap<String, Document>) -> Vec<DocEvent> {
        let mut events = vec![];
        while let Ok(done) = self.done.try_recv() {
            self.apply(done, docs, &mut events);
        }
        events
    }
    /// Blocks until every save requested so far, including coalesced follow-ups,
    /// has completed or failed (exit and window close).
    pub fn flush(&mut self, docs: &mut HashMap<String, Document>) -> Vec<DocEvent> {
        let mut events = self.poll(docs);
        while !self.saves.is_empty() {
            match self
                .done
                .recv_timeout(std::time::Duration::from_millis(100))
            {
                Ok(done) => self.apply(done, docs, &mut events),
                Err(_) => {
                    if self.thread.as_ref().is_none_or(|t| t.is_finished()) {
                        // The IO thread is gone (it panicked); nothing more will arrive.
                        events.extend(self.poll(docs));
                        for (id, _) in self.saves.drain() {
                            events.push(DocEvent::SaveFailed {
                                id,
                                error: "Document save worker stopped; the file was not saved"
                                    .into(),
                                conflict: false,
                            });
                        }
                    }
                }
            }
        }
        events
    }
    fn apply(
        &mut self,
        done: Done,
        docs: &mut HashMap<String, Document>,
        events: &mut Vec<DocEvent>,
    ) {
        match done {
            Done::Opened {
                id,
                request,
                result,
            } => {
                if self.opens.get(&id).is_none_or(|o| o.request != request) {
                    return; // superseded or forgotten
                }
                let Some(open) = self.opens.remove(&id) else {
                    return;
                };
                match result {
                    Ok(doc) => {
                        // Never clobber a document that appeared meanwhile (e.g. a restored draft).
                        docs.entry(id.clone()).or_insert(doc);
                        events.push(DocEvent::Opened { id });
                    }
                    Err(e) => {
                        let error = e.to_string();
                        self.failed.insert(id.clone(), error.clone());
                        events.push(DocEvent::OpenFailed {
                            id,
                            error,
                            remove_pane: open.remove_pane,
                        });
                    }
                }
            }
            Done::Saved {
                id,
                request,
                path,
                text,
                result,
            } => {
                if self.saves.get(&id).is_none_or(|s| s.request != request) {
                    return;
                }
                let Some(running) = self.saves.remove(&id) else {
                    return;
                };
                let doc = docs.get_mut(&id).filter(|d| d.path == path);
                match result {
                    Ok(hash) => {
                        let mut follow_up = false;
                        if let Some(doc) = doc {
                            doc.hash = hash;
                            // Edits made while saving keep the document dirty.
                            doc.original = text;
                            follow_up = running.queued && doc.dirty();
                            if follow_up {
                                let doc = doc.clone();
                                self.start_save(&id, &doc);
                            }
                        }
                        if !follow_up {
                            events.push(DocEvent::Saved { id, path });
                        }
                    }
                    Err(e) => {
                        let conflict = e.downcast_ref::<Conflict>().is_some();
                        events.push(DocEvent::SaveFailed {
                            id,
                            error: e.to_string(),
                            conflict,
                        });
                    }
                }
            }
        }
    }
}
impl Drop for DocumentIo {
    fn drop(&mut self) {
        // Closing the job channel lets the thread finish queued work, then exit.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
}
pub fn list(path: &Path, filter: &str) -> Result<Vec<Entry>> {
    let filter = filter.to_lowercase();
    let mut entries = vec![];
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.to_lowercase().contains(&filter) {
            // DirEntry::file_type needs no extra stat on most platforms; only a
            // symlink is followed to decide whether it points at a directory.
            let file_type = item.file_type()?;
            let path = item.path();
            let directory = file_type.is_dir() || (file_type.is_symlink() && path.is_dir());
            entries.push(Entry {
                path,
                name,
                directory,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_file_is_never_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "original").unwrap();
        let mut doc = Document::open(&p).unwrap();
        doc.text = "edited".into();
        fs::write(&p, "external").unwrap();
        assert!(doc.save().is_err());
        assert_eq!(fs::read_to_string(p).unwrap(), "external");
    }
    #[test]
    fn list_sorts_directories_first_and_filters() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("Zeta")).unwrap();
        fs::write(d.path().join("alpha.txt"), "a").unwrap();
        fs::write(d.path().join("Beta.rs"), "b").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("Zeta"), d.path().join("link")).unwrap();
        let names = |filter: &str| -> Vec<(String, bool)> {
            list(d.path(), filter)
                .unwrap()
                .into_iter()
                .map(|e| (e.name, e.directory))
                .collect()
        };
        #[cfg(unix)]
        assert_eq!(
            names(""),
            vec![
                ("link".into(), true),
                ("Zeta".into(), true),
                ("alpha.txt".into(), false),
                ("Beta.rs".into(), false),
            ]
        );
        assert_eq!(names("BET"), vec![("Beta.rs".into(), false)]);
        assert!(list(&d.path().join("missing"), "").is_err());
    }
    #[test]
    fn open_reads_utf8_and_hash_matches_save_check() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "héllo").unwrap();
        let mut doc = Document::open(&p).unwrap();
        assert!(!doc.read_only && !doc.dirty());
        assert_eq!(doc.text, "héllo");
        doc.text = "bye".into();
        doc.save().unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "bye");
    }
    #[test]
    fn unknown_encoding_is_read_only() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, [0xff, 0xe9]).unwrap();
        let mut doc = Document::open(&p).unwrap();
        assert!(doc.read_only);
        assert_eq!(doc.text, "\u{ff}\u{e9}");
        assert!(doc.save().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn save_preserves_symlink_and_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        let link = d.path().join("link");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&p, &link).unwrap();
        let mut doc = Document::open(&link).unwrap();
        doc.text = "new".into();
        doc.save().unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o640);
    }
    #[cfg(unix)]
    #[test]
    fn save_refuses_to_change_owner() {
        use std::os::unix::fs::MetadataExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "x").unwrap();
        let mut meta_src = fs::metadata(&p).unwrap();
        let temp = tempfile::NamedTempFile::new_in(d.path()).unwrap();
        // Same owner: nothing to do.
        preserve_owner(temp.as_file(), &p, &meta_src).unwrap();
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            // A file owned by root (uid 0) cannot be handed to us; save must refuse.
            meta_src = fs::metadata("/").unwrap();
            assert_eq!(meta_src.uid(), 0);
            let err = preserve_owner(temp.as_file(), &p, &meta_src).unwrap_err();
            assert!(err.to_string().contains("change its owner"), "{err}");
        }
        let mut doc = Document::open(&p).unwrap();
        doc.text = "y".into();
        doc.save().unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().uid(),
            fs::metadata(d.path()).unwrap().uid()
        );
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Draft {
    pub path: PathBuf,
    pub text: String,
    pub original_hash: Vec<u8>,
}
impl Document {
    pub fn draft(&self) -> Draft {
        Draft {
            path: self.path.clone(),
            text: self.text.clone(),
            original_hash: self.hash.clone(),
        }
    }
    /// Restores a draft only if it targets one of `editor_paths` (the editor panes of
    /// the loaded workspace). Paths are compared after canonicalization so a draft
    /// cannot make the app open a file the workspace never had open.
    pub fn restore(draft: Draft, editor_paths: &[PathBuf]) -> Result<Self> {
        let target = draft
            .path
            .canonicalize()
            .with_context(|| format!("Recovery target {} is unavailable", draft.path.display()))?;
        if !editor_paths
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .any(|p| p == target)
        {
            bail!(
                "Skipped recovery for {}: no editor pane in this workspace has it open. The recovery file has been preserved.",
                draft.path.display()
            );
        }
        let mut doc = Self::open(&target)?;
        if doc.hash != draft.original_hash {
            bail!(
                "Recovery for {} conflicts with changes on disk. The recovery file has been preserved.",
                draft.path.display()
            );
        }
        doc.text = draft.text;
        Ok(doc)
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn draft_restores_text_without_writing_and_rejects_conflicts() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("file");
        fs::write(&path, "original").unwrap();
        let mut doc = Document::open(&path).unwrap();
        doc.text = "unsaved".into();
        let draft = doc.draft();
        let editors = vec![path.clone()];
        let restored = Document::restore(draft, &editors).unwrap();
        assert_eq!(restored.text, "unsaved");
        assert!(restored.dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        fs::write(&path, "external edit").unwrap();
        assert!(Document::restore(doc.draft(), &editors).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "external edit");
    }
    #[test]
    fn draft_for_path_without_editor_pane_is_skipped() {
        let d = tempfile::tempdir().unwrap();
        let open = d.path().join("open.txt");
        let secret = d.path().join("secret.txt");
        fs::write(&open, "a").unwrap();
        fs::write(&secret, "s").unwrap();
        // A draft naming a file the workspace never opened, with a matching hash.
        let draft = Draft {
            path: secret.clone(),
            text: "x".into(),
            original_hash: Sha256::digest(b"s").to_vec(),
        };
        let err = Document::restore(draft, std::slice::from_ref(&open))
            .err()
            .unwrap();
        assert!(err.to_string().contains("Skipped recovery"), "{err}");
        assert!(
            Document::restore(
                Draft {
                    path: secret,
                    text: "x".into(),
                    original_hash: Sha256::digest(b"s").to_vec(),
                },
                &[]
            )
            .is_err()
        );
        // Non-canonical spellings of an open editor path still match.
        let draft = Draft {
            path: d.path().join(".").join("open.txt"),
            text: "b".into(),
            original_hash: Sha256::digest(b"a").to_vec(),
        };
        let editors = vec![d.path().join("sub/../open.txt")];
        fs::create_dir(d.path().join("sub")).unwrap();
        assert_eq!(Document::restore(draft, &editors).unwrap().text, "b");
    }
}

#[cfg(test)]
mod io_tests {
    use super::*;
    fn io() -> DocumentIo {
        DocumentIo::new(|| {})
    }
    fn opened(io: &mut DocumentIo, docs: &mut HashMap<String, Document>, id: &str, path: &Path) {
        io.open(id, path, false);
        while io.loading(id) {
            io.poll(docs);
            std::thread::yield_now();
        }
        assert!(docs.contains_key(id), "{:?}", io.open_error(id));
    }
    #[test]
    fn concurrent_saves_coalesce_and_latest_text_wins() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "v0").unwrap();
        let mut io = io();
        let mut docs = HashMap::new();
        opened(&mut io, &mut docs, "e", &p);
        for (i, text) in ["v1", "v2", "v3"].into_iter().enumerate() {
            docs.get_mut("e").unwrap().text = text.into();
            let expected = if i == 0 {
                SaveRequest::Started
            } else {
                SaveRequest::Queued
            };
            assert_eq!(io.save("e", &docs["e"]), expected);
            assert!(io.saving("e"));
        }
        let events = io.flush(&mut docs);
        assert!(!io.saving("e"));
        // One save for v1, one coalesced follow-up for the latest text; never two at once.
        assert_eq!(io.saves_submitted(), 2);
        assert_eq!(fs::read_to_string(&p).unwrap(), "v3");
        assert!(!docs["e"].dirty());
        assert_eq!(
            events,
            vec![DocEvent::Saved {
                id: "e".into(),
                path: docs["e"].path.clone()
            }]
        );
        // The follow-up's hash is the new baseline: a later save does not conflict.
        docs.get_mut("e").unwrap().text = "v4".into();
        io.save("e", &docs["e"]);
        io.flush(&mut docs);
        assert_eq!(fs::read_to_string(&p).unwrap(), "v4");
    }
    #[test]
    fn edit_during_save_leaves_document_dirty() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "v0").unwrap();
        let mut io = io();
        let mut docs = HashMap::new();
        opened(&mut io, &mut docs, "e", &p);
        docs.get_mut("e").unwrap().text = "saved".into();
        io.save("e", &docs["e"]);
        docs.get_mut("e").unwrap().text = "typed later".into();
        io.flush(&mut docs);
        assert_eq!(fs::read_to_string(&p).unwrap(), "saved");
        assert!(docs["e"].dirty());
        assert_eq!(docs["e"].original, "saved");
    }
    #[test]
    fn background_save_still_detects_conflicts() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "original").unwrap();
        let mut io = io();
        let mut docs = HashMap::new();
        opened(&mut io, &mut docs, "e", &p);
        docs.get_mut("e").unwrap().text = "edited".into();
        fs::write(&p, "external").unwrap();
        io.save("e", &docs["e"]);
        // A queued follow-up must not retry past a conflict.
        io.save("e", &docs["e"]);
        let events = io.flush(&mut docs);
        assert_eq!(
            events,
            vec![DocEvent::SaveFailed {
                id: "e".into(),
                error: "File changed on disk. Reopen it before saving.".into(),
                conflict: true,
            }]
        );
        assert_eq!(io.saves_submitted(), 1);
        assert_eq!(fs::read_to_string(&p).unwrap(), "external");
        assert!(docs["e"].dirty());
        // The baseline is unchanged, so retrying still refuses.
        io.save("e", &docs["e"]);
        assert!(matches!(
            io.flush(&mut docs).as_slice(),
            [DocEvent::SaveFailed { conflict: true, .. }]
        ));
        assert_eq!(fs::read_to_string(&p).unwrap(), "external");
    }
    #[test]
    fn stale_open_result_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("old");
        let new = d.path().join("new");
        fs::write(&old, "old").unwrap();
        fs::write(&new, "new").unwrap();
        let mut io = io();
        let mut docs = HashMap::new();
        io.open("e", &old, false);
        io.open("e", &new, false);
        while io.loading("e") {
            io.poll(&mut docs);
            std::thread::yield_now();
        }
        assert_eq!(docs["e"].text, "new");
        // A result for a forgotten pane is dropped too.
        io.open("gone", &old, false);
        io.retain(|id| id != "gone");
        std::thread::sleep(std::time::Duration::from_millis(50));
        io.poll(&mut docs);
        assert!(!docs.contains_key("gone"));
    }
    #[test]
    fn open_failure_is_reported_once_and_remembered() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bin");
        fs::write(&p, [0u8, 1, 2]).unwrap();
        let mut io = io();
        let mut docs = HashMap::new();
        io.open("e", &p, true);
        let mut events = vec![];
        while io.loading("e") {
            events.extend(io.poll(&mut docs));
            std::thread::yield_now();
        }
        assert_eq!(
            events,
            vec![DocEvent::OpenFailed {
                id: "e".into(),
                error: "Binary file; use an external application".into(),
                remove_pane: true,
            }]
        );
        assert_eq!(
            io.open_error("e"),
            Some("Binary file; use an external application")
        );
        assert!(docs.is_empty());
    }
    #[test]
    fn dropping_the_worker_completes_queued_saves() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, "v0").unwrap();
        let mut docs = HashMap::new();
        {
            let mut io = io();
            opened(&mut io, &mut docs, "e", &p);
            docs.get_mut("e").unwrap().text = "v1".into();
            io.save("e", &docs["e"]);
        }
        assert_eq!(fs::read_to_string(&p).unwrap(), "v1");
    }
}
