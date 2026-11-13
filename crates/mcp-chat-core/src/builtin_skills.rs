//! Skills that ship with Jarvis and the installer that seeds them globally.
//!
//! The files are embedded in the binary so a packaged app does not depend on a
//! separate resources directory. On first launch they are copied into the
//! global skill root (`~/.config/mcp-chat/skills`). A small manifest records
//! which files Jarvis owns, so an app update can refresh untouched built-ins
//! without overwriting a skill the user edited.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

struct BundledSkill {
    name: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

const BUILTIN_SKILLS: &[BundledSkill] = &[
    BundledSkill {
        name: "excalidraw",
        files: &[
            (
                "SKILL.md",
                include_bytes!("../../../skills/excalidraw/SKILL.md"),
            ),
            (
                "references/colors.md",
                include_bytes!("../../../skills/excalidraw/references/colors.md"),
            ),
            (
                "references/dark-mode.md",
                include_bytes!("../../../skills/excalidraw/references/dark-mode.md"),
            ),
            (
                "references/examples.md",
                include_bytes!("../../../skills/excalidraw/references/examples.md"),
            ),
            (
                "scripts/upload.py",
                include_bytes!("../../../skills/excalidraw/scripts/upload.py"),
            ),
        ],
    },
    BundledSkill {
        name: "manage-skill",
        files: &[(
            "SKILL.md",
            include_bytes!("../../../skills/manage-skill/SKILL.md"),
        )],
    },
    BundledSkill {
        name: "spreadsheet",
        files: &[
            (
                "SKILL.md",
                include_bytes!("../../../skills/spreadsheet/SKILL.md"),
            ),
            (
                "INSTALL.md",
                include_bytes!("../../../skills/spreadsheet/INSTALL.md"),
            ),
            (
                "scripts/diff.py",
                include_bytes!("../../../skills/spreadsheet/scripts/diff.py"),
            ),
            (
                "scripts/probe.py",
                include_bytes!("../../../skills/spreadsheet/scripts/probe.py"),
            ),
        ],
    },
];

const MANIFEST_FILE: &str = ".jarvis-builtins.json";
const MANIFEST_VERSION: u8 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    version: u8,
    #[serde(default)]
    skills: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct InstallReport {
    pub installed: usize,
    pub updated: usize,
    pub preserved: usize,
    pub failed: usize,
}

impl InstallReport {
    pub fn changed(&self) -> bool {
        self.installed > 0 || self.updated > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOutcome {
    Installed,
    Updated,
    Unchanged,
    Preserved,
    Failed,
}

pub fn install_global_skills() -> Option<InstallReport> {
    let root = crate::agent::tools::global_skills_dir()?;
    Some(install_builtin_skills(&root))
}

/// Install or refresh the bundled skills in an explicit global skill directory.
pub fn install_builtin_skills(root: &Path) -> InstallReport {
    let mut report = InstallReport::default();
    let mut manifest = read_manifest(root);
    let mut next_manifest = Manifest {
        version: MANIFEST_VERSION,
        skills: BTreeMap::new(),
    };

    for skill in BUILTIN_SKILLS {
        let mut owned = manifest.skills.remove(skill.name).unwrap_or_default();
        for (relative, bytes) in skill.files {
            if !safe_relative(relative) {
                report.failed += 1;
                continue;
            }
            let target = root.join(skill.name).join(relative);
            let bundled_hash = content_hash(bytes);
            let outcome = install_file(&target, bytes, owned.get(*relative).map(String::as_str));
            match outcome {
                FileOutcome::Installed => {
                    report.installed += 1;
                    owned.insert((*relative).to_owned(), bundled_hash);
                }
                FileOutcome::Updated => {
                    report.updated += 1;
                    owned.insert((*relative).to_owned(), bundled_hash);
                }
                FileOutcome::Unchanged => {
                    owned.insert((*relative).to_owned(), bundled_hash);
                }
                FileOutcome::Preserved => {
                    report.preserved += 1;
                    owned.remove(*relative);
                }
                FileOutcome::Failed => {
                    report.failed += 1;
                }
            }
        }
        if !owned.is_empty() {
            next_manifest.skills.insert(skill.name.to_owned(), owned);
        }
    }

    if let Ok(bytes) = serde_json::to_vec_pretty(&next_manifest)
        && fs::write(root.join(MANIFEST_FILE), bytes).is_err()
    {
        report.failed += 1;
    }
    report
}

fn install_file(target: &Path, bytes: &[u8], previous_hash: Option<&str>) -> FileOutcome {
    let existing = match fs::read(target) {
        Ok(existing) => Some(existing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return FileOutcome::Failed,
    };
    let Some(existing) = existing else {
        return if write_file(target, bytes).is_ok() {
            FileOutcome::Installed
        } else {
            FileOutcome::Failed
        };
    };
    let existing_hash = content_hash(&existing);
    let bundled_hash = content_hash(bytes);
    if existing_hash == bundled_hash {
        FileOutcome::Unchanged
    } else if previous_hash == Some(existing_hash.as_str()) {
        if write_file(target, bytes).is_ok() {
            FileOutcome::Updated
        } else {
            FileOutcome::Failed
        }
    } else {
        FileOutcome::Preserved
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

fn read_manifest(root: &Path) -> Manifest {
    let Ok(bytes) = fs::read(root.join(MANIFEST_FILE)) else {
        return Manifest {
            version: MANIFEST_VERSION,
            skills: BTreeMap::new(),
        };
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
        return Manifest {
            version: MANIFEST_VERSION,
            skills: BTreeMap::new(),
        };
    };
    if manifest.version == MANIFEST_VERSION {
        manifest
    } else {
        Manifest {
            version: MANIFEST_VERSION,
            skills: BTreeMap::new(),
        }
    }
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.split(['/', '\\']).any(|part| part == "..")
}

fn content_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_skill_has_a_valid_manifest() {
        let mut names = std::collections::HashSet::new();
        for skill in BUILTIN_SKILLS {
            assert!(names.insert(skill.name), "duplicate skill {}", skill.name);
            let (path, bytes) = skill
                .files
                .iter()
                .find(|(path, _)| *path == "SKILL.md")
                .expect("missing SKILL.md");
            assert_eq!(*path, "SKILL.md");
            let text = std::str::from_utf8(bytes).expect("SKILL.md must be UTF-8");
            assert!(
                text.starts_with("---\n"),
                "{} has no frontmatter",
                skill.name
            );
            assert!(
                text.contains(&format!("name: {}", skill.name)),
                "{} frontmatter name does not match",
                skill.name
            );
            assert!(
                text.contains("description:"),
                "{} has no description",
                skill.name
            );
            for (relative, _) in skill.files {
                assert!(safe_relative(relative), "unsafe path {relative}");
            }
        }
    }

    #[test]
    fn installs_all_builtin_skills_and_preserves_user_edits() {
        let root = tempfile::tempdir().unwrap().path().join("skills");
        let report = install_builtin_skills(&root);
        assert_eq!(report.failed, 0);
        assert_eq!(report.installed, 10);
        for skill in BUILTIN_SKILLS {
            assert!(
                root.join(skill.name).join("SKILL.md").is_file(),
                "missing {}",
                skill.name
            );
        }
        assert!(root.join(MANIFEST_FILE).is_file());

        let editable = root.join("excalidraw").join("SKILL.md");
        let original = fs::read_to_string(&editable).unwrap();
        assert!(original.contains("Excalidraw Diagram Skill"));
        fs::write(&editable, "my local edit\n").unwrap();

        let report = install_builtin_skills(&root);
        assert_eq!(report.updated, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(fs::read_to_string(&editable).unwrap(), "my local edit\n");
    }

    #[test]
    fn installer_repairs_missing_owned_files() {
        let root = tempfile::tempdir().unwrap().path().join("skills");
        assert_eq!(install_builtin_skills(&root).failed, 0);
        let target = root.join("excalidraw").join("references/colors.md");
        fs::remove_file(&target).unwrap();
        let report = install_builtin_skills(&root);
        assert_eq!(report.installed, 1);
        assert_eq!(report.failed, 0);
        assert!(target.is_file());
    }

    #[test]
    fn installer_refreshes_an_untouched_old_file() {
        let root = tempfile::tempdir().unwrap().path().join("skills");
        assert_eq!(install_builtin_skills(&root).failed, 0);
        let relative = "references/colors.md";
        let target = root.join("excalidraw").join(relative);
        let old = b"old bundled version\n";
        fs::write(&target, old).unwrap();

        let manifest_path = root.join(MANIFEST_FILE);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["skills"]["excalidraw"][relative] = serde_json::Value::String(content_hash(old));
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let report = install_builtin_skills(&root);
        assert_eq!(report.updated, 1);
        assert_eq!(report.failed, 0);
        let expected = BUILTIN_SKILLS
            .iter()
            .find(|skill| skill.name == "excalidraw")
            .and_then(|skill| skill.files.iter().find(|(path, _)| *path == relative))
            .map(|(_, bytes)| *bytes)
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), expected);
    }

    #[test]
    fn installer_does_not_take_over_an_existing_unowned_skill() {
        let root = tempfile::tempdir().unwrap().path().join("skills");
        let target = root.join("excalidraw").join("SKILL.md");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "manual install\n").unwrap();

        let report = install_builtin_skills(&root);
        assert_eq!(report.failed, 0);
        assert_eq!(fs::read_to_string(&target).unwrap(), "manual install\n");
        assert!(root.join("excalidraw/references/colors.md").is_file());
    }
}
