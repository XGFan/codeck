//! Project identity: the git repository a directory belongs to (a git worktree folds into its main
//! repository), else the directory itself. Only `stat` and reading `.git` files, never writes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::index::writer::ProjectRow;

/// Tools whose `file_path` / `notebook_path` input counts as an edit of that file's Project.
pub const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Resolves directories to Projects, memoizing filesystem lookups (one instance per sync).
pub struct Resolver {
    /// `$HOME` is never a repository root (a dotfiles repo would swallow every directory below it).
    home: Option<PathBuf>,
    /// Edits under this directory (the data root: plans, memory) are not Project edits.
    excluded: Option<PathBuf>,
    roots: HashMap<PathBuf, Option<PathBuf>>,
    /// Directory → realpath of it or its nearest existing ancestor.
    existing: HashMap<PathBuf, Option<PathBuf>>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new(std::env::var_os("HOME").map(PathBuf::from), None)
    }
}

impl Resolver {
    pub fn new(home: Option<PathBuf>, excluded: Option<&Path>) -> Self {
        let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        Self {
            home: home.as_deref().map(real),
            excluded: excluded.map(real),
            roots: HashMap::new(),
            existing: HashMap::new(),
        }
    }

    /// [`Resolver::default`] that also ignores edits under `data_root`.
    pub fn for_data_root(data_root: &Path) -> Self {
        Self::new(std::env::var_os("HOME").map(PathBuf::from), Some(data_root))
    }

    /// Project a Session started in: the repository of the realpath of its start cwd, else that
    /// realpath. When the directory no longer exists: the repository of its nearest existing
    /// ancestor (a removed worktree or subdirectory), else the raw cwd flagged missing. The encoded
    /// project dir when no entry carries a cwd.
    pub fn start_project(&mut self, cwd: Option<&str>, main_file: &Path) -> ProjectRow {
        match cwd.filter(|c| !c.is_empty()) {
            Some(c) => match fs::canonicalize(c) {
                Ok(real) => row(&self.repo_root(&real).unwrap_or(real), false),
                Err(_) => match self
                    .existing_ancestor(Path::new(c))
                    .and_then(|a| self.repo_root(&a))
                {
                    Some(root) => row(&root, false),
                    None => row(Path::new(c), true),
                },
            },
            None => row(main_file.parent().unwrap_or(main_file), false),
        }
    }

    /// Project of an edited file: its repository; the start Project for a file under it that is in
    /// no repository; `None` otherwise (relative paths, files outside any repository, files under
    /// the data root, …).
    pub fn edit_project(&mut self, file: &str, start: &ProjectRow) -> Option<ProjectRow> {
        let file = Path::new(file);
        if !file.is_absolute() {
            return None;
        }
        // The file itself may be gone; resolve its nearest existing ancestor.
        let real = self.existing_ancestor(file.parent()?)?;
        if self
            .excluded
            .as_deref()
            .is_some_and(|x| real.starts_with(x))
        {
            return None;
        }
        if let Some(root) = self.repo_root(&real) {
            return Some(row(&root, false));
        }
        (!start.missing && real.starts_with(&start.path)).then(|| start.clone())
    }

    /// Realpath of `dir` or of its nearest existing ancestor.
    fn existing_ancestor(&mut self, dir: &Path) -> Option<PathBuf> {
        if let Some(r) = self.existing.get(dir) {
            return r.clone();
        }
        let found = dir.ancestors().find_map(|a| fs::canonicalize(a).ok());
        self.existing.insert(dir.to_path_buf(), found.clone());
        found
    }

    /// Nearest repository root at or above `dir` (a realpath), below `$HOME`.
    fn repo_root(&mut self, dir: &Path) -> Option<PathBuf> {
        if let Some(r) = self.roots.get(dir) {
            return r.clone();
        }
        let mut found = None;
        for a in dir.ancestors() {
            if a.parent().is_none() || self.home.as_deref() == Some(a) {
                break;
            }
            let git = a.join(".git");
            if let Ok(meta) = fs::metadata(&git) {
                found = Some(if meta.is_file() {
                    worktree_main(a, &git)
                } else {
                    a.to_path_buf()
                });
                break;
            }
        }
        self.roots.insert(dir.to_path_buf(), found.clone());
        found
    }
}

/// The main repository of a linked worktree (`.git` file reading `gitdir: <main>/.git/worktrees/<name>`);
/// `dir` itself for anything else, such as a submodule.
fn worktree_main(dir: &Path, git_file: &Path) -> PathBuf {
    let Some(gitdir) = fs::read_to_string(git_file)
        .ok()
        .and_then(|s| s.trim().strip_prefix("gitdir:").map(|g| dir.join(g.trim())))
    else {
        return dir.to_path_buf();
    };
    let worktrees = gitdir.parent();
    match worktrees.and_then(Path::parent) {
        Some(dot_git)
            if worktrees.and_then(Path::file_name) == Some("worktrees".as_ref())
                && dot_git.file_name() == Some(".git".as_ref()) =>
        {
            let main = dot_git.parent().unwrap_or(dot_git);
            fs::canonicalize(main).unwrap_or_else(|_| main.to_path_buf())
        }
        _ => dir.to_path_buf(),
    }
}

fn row(path: &Path, missing: bool) -> ProjectRow {
    let id = path.to_string_lossy().into_owned();
    let display_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| id.clone());
    ProjectRow {
        path: id.clone(),
        id,
        display_name,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<tmp>/home/{ws/{api/.git/, api/src/, notes/}, wt/ (worktree of api)}`.
    fn layout() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = fs::canonicalize(tmp.path()).unwrap().join("home");
        fs::create_dir_all(home.join("ws/api/.git/worktrees/wt")).unwrap();
        fs::create_dir_all(home.join("ws/api/src")).unwrap();
        fs::create_dir_all(home.join("ws/notes")).unwrap();
        fs::create_dir_all(home.join("wt/src")).unwrap();
        let gitdir = home.join("ws/api/.git/worktrees/wt");
        fs::write(
            home.join("wt/.git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .unwrap();
        (tmp, home)
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn start_project_is_the_repository_or_the_directory() {
        let (_t, home) = layout();
        let mut r = Resolver::new(Some(home.clone()), None);
        let main = Path::new("/x/projects/-enc/s.jsonl");
        assert_eq!(
            r.start_project(Some(&s(&home.join("ws/api/src"))), main).id,
            s(&home.join("ws/api"))
        );
        let ws = r.start_project(Some(&s(&home.join("ws"))), main);
        assert_eq!(
            (ws.id.as_str(), ws.display_name.as_str(), ws.missing),
            (s(&home.join("ws")).as_str(), "ws", false)
        );
        let gone = r.start_project(Some("/no/such/dir"), main);
        assert_eq!((gone.id.as_str(), gone.missing), ("/no/such/dir", true));
        assert_eq!(r.start_project(None, main).id, "/x/projects/-enc");
    }

    #[test]
    fn worktree_folds_into_its_main_repository() {
        let (_t, home) = layout();
        let mut r = Resolver::new(Some(home.clone()), None);
        let start = r.start_project(Some(&s(&home.join("wt/src"))), Path::new("/m"));
        assert_eq!(start.id, s(&home.join("ws/api")));
    }

    #[test]
    fn edit_project_resolves_repositories_and_falls_back_to_the_start_project() {
        let (_t, home) = layout();
        let mut r = Resolver::new(Some(home.clone()), None);
        let start = r.start_project(Some(&s(&home.join("ws"))), Path::new("/m"));
        let edit = |r: &mut Resolver, p: &str| r.edit_project(p, &start).map(|p| p.id);
        // A file that no longer exists still resolves through its directory.
        assert_eq!(
            edit(&mut r, &s(&home.join("ws/api/src/gone/new.rs"))),
            Some(s(&home.join("ws/api")))
        );
        assert_eq!(
            edit(&mut r, &s(&home.join("ws/notes/todo.md"))),
            Some(start.id.clone())
        );
        assert_eq!(edit(&mut r, &s(&home.join("elsewhere.md"))), None);
        assert_eq!(edit(&mut r, "relative/file.rs"), None);
    }

    #[test]
    fn removed_start_directory_inside_a_repository_belongs_to_it() {
        let (_t, home) = layout();
        let mut r = Resolver::new(Some(home.clone()), None);
        let gone = r.start_project(
            Some(&s(&home.join("ws/api/.claude/worktrees/feat"))),
            Path::new("/m"),
        );
        assert_eq!((gone.id, gone.missing), (s(&home.join("ws/api")), false));
    }

    #[test]
    fn edits_under_the_excluded_directory_do_not_count() {
        let (_t, home) = layout();
        let mut r = Resolver::new(Some(home.clone()), Some(&home.join("ws/api/src")));
        let start = r.start_project(Some(&s(&home.join("ws"))), Path::new("/m"));
        assert_eq!(
            r.edit_project(&s(&home.join("ws/api/src/plan.md")), &start),
            None
        );
        assert!(
            r.edit_project(&s(&home.join("ws/api/README.md")), &start)
                .is_some()
        );
    }

    #[test]
    fn home_is_never_a_repository_root() {
        let (_t, home) = layout();
        fs::create_dir_all(home.join(".git")).unwrap();
        let mut r = Resolver::new(Some(home.clone()), None);
        let ws = r.start_project(Some(&s(&home.join("ws/notes"))), Path::new("/m"));
        assert_eq!(ws.id, s(&home.join("ws/notes")));
    }
}
