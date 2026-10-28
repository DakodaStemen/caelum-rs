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
        temp.as_file().set_permissions(meta.permissions())?;
        temp.write_all(self.text.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)?;
        self.hash = Sha256::digest(self.text.as_bytes()).to_vec();
        self.original = self.text.clone();
        Ok(())
    }
}
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
            entries.push(Entry {
                path: item.path(),
                name,
                directory: item.path().is_dir(),
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
    pub fn restore(draft: Draft) -> Result<Self> {
        let mut doc = Self::open(&draft.path)?;
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
        let restored = Document::restore(draft).unwrap();
        assert_eq!(restored.text, "unsaved");
        assert!(restored.dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        fs::write(&path, "external edit").unwrap();
        assert!(Document::restore(doc.draft()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "external edit");
    }
}
