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

        // Serialise on the UI thread (cheap); the background writer does the file
        // write and fsync so a slow disk never stalls a frame.
        let mut batch = vec![];
        match self.state.to_bytes() {
            Ok(bytes) => batch.push((self.profile.join("workspace.json"), bytes)),
            Err(e) => {
                self.message = format!("Workspace was not saved: {e}");
                return;
            }
        }
        if !self.recovery_conflict {
            let drafts: Vec<_> = self
                .documents
                .values()
                .filter(|d| d.dirty())
                .map(Document::draft)
                .collect();
            match serde_json::to_vec_pretty(&drafts) {
                Ok(bytes) => batch.push((self.profile.join("drafts.json"), bytes)),
                Err(e) => self.message = format!("Recovery was not saved: {e}"),
            }
        }
        self.writer.submit(batch);
        self.last_save = Instant::now();
    }
    /// Finishes pending document saves, then persists and waits until the files are
    /// on disk (exit, window close and smoke checks).
    pub(super) fn persist_now(&mut self) {
        // Drafts reflect what the saves left dirty, so they are taken afterwards.
        self.flush_documents();
        self.persist();
        self.writer.flush();
        self.report_write_errors();
    }
    /// Surfaces failures from the background writer in the status line.
    pub(super) fn report_write_errors(&mut self) {
        for e in self.writer.take_errors() {
            eprintln!("caelum-rs: {e}");
            self.message = e;
        }
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
                // Only paths already open in an editor pane of the loaded workspace may be
                // restored; a draft never makes the app open a new file.
                let mut editors: Vec<(String, PathBuf)> = vec![];
                for w in &self.state.workspaces {
                    for t in &w.tabs {
                        let mut leaves = vec![];
                        t.layout.leaves(&mut leaves);
                        for p in leaves {
                            if let Pane::Editor { id, path } = p {
                                editors.push((id.clone(), path.clone()));
                            }
                        }
                    }
                }
                let paths: Vec<PathBuf> = editors.iter().map(|(_, p)| p.clone()).collect();
                for draft in drafts {
                    match Document::restore(draft, &paths) {
                        Ok(doc) => {
                            for (id, path) in &editors {
                                if path.canonicalize().ok().as_ref() == Some(&doc.path) {
                                    self.documents.insert(id.clone(), doc.clone());
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("caelum-rs: {e}");
                            self.message = e.to_string();
                            // Keeps drafts.json: it is copied aside before the next save.
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
