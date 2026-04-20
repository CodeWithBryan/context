use std::path::{Path, PathBuf};

/// Directory names that should never trigger indexing.
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    ".next",
    "build",
    ".turbo",
    ".cache",
    ".parcel-cache",
    ".vercel",
    ".svelte-kit",
    "out",
    ".ctx", // our own per-repo state dir
];

#[must_use]
pub fn is_ignored(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| EXCLUDED_DIRS.contains(&s))
    })
}

/// Dedupe a batch of paths, drop ignored, return canonical list.
#[must_use]
pub fn clean_batch(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        if is_ignored(&p) {
            continue;
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ignores_node_modules() {
        let p = PathBuf::from("/project/node_modules/pkg/index.ts");
        assert!(is_ignored(&p));
    }

    #[test]
    fn ignores_git_dir() {
        let p = PathBuf::from("/project/.git/HEAD");
        assert!(is_ignored(&p));
    }

    #[test]
    fn allows_src_files() {
        let p = PathBuf::from("/project/src/app.ts");
        assert!(!is_ignored(&p));
    }

    #[test]
    fn clean_batch_dedupes_and_filters() {
        let paths = vec![
            PathBuf::from("/project/src/a.ts"),
            PathBuf::from("/project/node_modules/pkg/index.ts"),
            PathBuf::from("/project/src/a.ts"), // duplicate
        ];
        let result = clean_batch(paths);
        assert_eq!(result, vec![PathBuf::from("/project/src/a.ts")]);
    }
}
