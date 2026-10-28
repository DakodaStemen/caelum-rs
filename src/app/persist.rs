use super::*;

/// Restricts a profile file to the owner on unix; a no-op elsewhere.
fn restrict(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Copies `source` to `backup`, creating the backup owner-only (0600) on unix.
fn copy_private(source: &Path, backup: &Path) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut input = fs::File::open(source)?;
    let mut output = options.open(backup)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    restrict(backup)
}

impl App {
    pub(super) fn persist(&mut self) {
        fn sync_cwd(p: &mut Pane, sessions: &HashMap<String, Session>) {
            match p {
                Pane::Terminal { id, cwd, .. } => {
                    if let Some(s) = sessions.get(id) {
                        *cwd = s.cwd.clone();
                    }
                }
                Pane::Split { first, second, .. } => {
                    sync_cwd(first, sessions);
                    sync_cwd(second, sessions);
                }
                _ => {}
            }
        }
        for w in &mut self.state.workspaces {
            for t in &mut w.tabs {
                sync_cwd(&mut t.layout, &self.terminals);
            }
        }
        if self.recovery_conflict {
            let source = self.profile.join("drafts.json");
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let backup = self.profile.join(format!("drafts-conflict-{stamp}.json"));
            match copy_private(&source, &backup) {
                Ok(()) => {
                    self.recovery_conflict = false;
                    self.message =
                        format!("Conflicting recovery preserved at {}", backup.display());
                }
                Err(e) => {
                    self.message = format!("Cannot preserve recovery: {e}");
                    return;
                }
            }
        }

        if let Err(e) = self.state.save(&self.profile.join("workspace.json")) {
            self.message = format!("Workspace was not saved: {e}");
            return;
        }
        if let Err(e) = restrict(&self.profile.join("workspace.json")) {
            self.message = format!("Workspace permissions were not restricted: {e}");
        }
        if !self.recovery_conflict {
            let drafts: Vec<_> = self
                .documents
                .values()
                .filter(|d| d.dirty())
                .map(Document::draft)
                .collect();
            let result = (|| -> Result<()> {
                let mut temp = tempfile::NamedTempFile::new_in(&self.profile)?;
                temp.write_all(&serde_json::to_vec_pretty(&drafts)?)?;
                restrict(temp.path())?;
                temp.as_file().sync_all()?;
                temp.persist(self.profile.join("drafts.json"))?;
                Ok(())
            })();
            if let Err(e) = result {
                self.message = format!("Recovery was not saved: {e}");
            }
        }
        self.last_save = Instant::now();
    }
    pub(super) fn restore_drafts(&mut self) {
        let file = self.profile.join("drafts.json");
        if !file.exists() {
            return;
        }
        let result =
            (|| -> Result<Vec<Draft>> { Ok(serde_json::from_slice(&fs::read(&file)?)?) })();
        match result {
            Ok(drafts) => {
                for draft in drafts {
                    match Document::restore(draft) {
                        Ok(doc) => {
                            let mut restored = false;
                            for w in &self.state.workspaces {
                                for t in &w.tabs {
                                    let mut leaves = vec![];
                                    t.layout.leaves(&mut leaves);
                                    for p in leaves {
                                        if let Pane::Editor { id, path } = p
                                            && path == &doc.path
                                        {
                                            self.documents.insert(id.clone(), doc.clone());
                                            restored = true;
                                        }
                                    }
                                }
                            }
                            if !restored {
                                let id = model::id();
                                let path = doc.path.clone();
                                self.documents.insert(id.clone(), doc);
                                self.insert(Pane::Editor { id, path });
                            }
                        }
                        Err(e) => {
                            self.message = e.to_string();
                            self.recovery_conflict = true;
                        }
                    }
                }
            }
            Err(e) => {
                self.message = format!(
                    "Recovery could not be read; preserved at {}: {e}",
                    file.display()
                );
                self.recovery_conflict = true;
            }
        }
    }
}
