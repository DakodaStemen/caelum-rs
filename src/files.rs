use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
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
        let (text, read_only) = match String::from_utf8(bytes.clone()) {
            Ok(t) => (t, false),
            Err(_) => (bytes.iter().map(|b| char::from(*b)).collect(), true),
        };
        Ok(Self {
            path,
            text: text.clone(),
            original: text,
            read_only,
            hash: Sha256::digest(&bytes).to_vec(),
        })
    }
    pub fn dirty(&self) -> bool {
        self.text != self.original
    }
    pub fn save(&mut self) -> Result<()> {
        if self.read_only {
            bail!("Unknown encoding: file is read-only");
        }
        if self.text.len() as u64 > LIMIT {
            bail!("File exceeds 5 MB");
        }
        let current = fs::read(&self.path)?;
        if Sha256::digest(&current).as_slice() != self.hash.as_slice() {
            bail!("File changed on disk. Reopen it before saving.");
        }
        let meta = fs::metadata(&self.path)?;
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
        let mut temp =
            tempfile::NamedTempFile::new_in(self.path.parent().context("No parent directory")?)?;
        #[cfg(unix)]
        preserve_owner(temp.as_file(), &self.path, &meta)?;
        temp.as_file().set_permissions(meta.permissions())?;
        temp.write_all(self.text.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)?;
        self.hash = Sha256::digest(self.text.as_bytes()).to_vec();
        self.original = self.text.clone();
        Ok(())
    }
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
