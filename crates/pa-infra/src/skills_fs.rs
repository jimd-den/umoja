//! Skill discovery across every location, in precedence order.
//!
//! The locations deliberately include Claude Code's and the cross-harness
//! `~/.agents/skills`, so a skill written once is visible to every tool on the
//! machine rather than to whichever one happened to install it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pa_domain::error::{DomainError, Result};
use pa_domain::ports::SkillCatalog;
use pa_domain::skill::{SkillKind, SkillManifest, SkillSource, Validation};

use crate::paths::home_dir;

/// Directory names searched at each scope.
const PROJECT_DIRS: [&str; 3] = [".prime/agent/skills", ".agents/skills", ".claude/skills"];
const PERSONAL_DIRS: [&str; 3] = [
    ".prime/agent/skills",
    ".agents/skills",
    ".claude/skills",
];

/// How far up the tree to look for project skills.
const MAX_ANCESTORS: usize = 12;

pub struct FsSkillCatalog {
    /// Extra roots, from `--skill` or settings. Highest precedence.
    explicit: Vec<PathBuf>,
    builtin: Option<PathBuf>,
    /// Where personal skills live. Injected rather than read from the
    /// environment so a test can point it somewhere harmless.
    home: Option<PathBuf>,
}

impl std::fmt::Debug for FsSkillCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FsSkillCatalog")
    }
}

impl Default for FsSkillCatalog {
    fn default() -> Self {
        Self::new(Vec::new(), None)
    }
}

impl FsSkillCatalog {
    pub fn new(explicit: Vec<PathBuf>, builtin: Option<PathBuf>) -> Self {
        Self {
            explicit,
            builtin,
            home: home_dir(),
        }
    }

    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    fn roots(&self, workdir: &str) -> Vec<(SkillSource, PathBuf)> {
        let mut roots: Vec<(SkillSource, PathBuf)> = Vec::new();

        for path in &self.explicit {
            roots.push((SkillSource::Explicit, path.clone()));
        }

        // Project skills, walking up towards the repository root so a skill in
        // the repo applies to every directory inside it.
        let mut cursor = Some(PathBuf::from(workdir));
        let mut climbed = 0;
        while let Some(dir) = cursor {
            for name in PROJECT_DIRS {
                let candidate = dir.join(name);
                if candidate.is_dir() {
                    roots.push((SkillSource::Project, candidate));
                }
            }
            if dir.join(".git").exists() || climbed >= MAX_ANCESTORS {
                break;
            }
            climbed += 1;
            cursor = dir.parent().map(Path::to_path_buf);
        }

        if let Some(home) = self.home.clone() {
            for name in PERSONAL_DIRS {
                let candidate = home.join(name);
                if candidate.is_dir() {
                    roots.push((SkillSource::Personal, candidate));
                }
            }
        }

        if let Some(builtin) = &self.builtin {
            roots.push((SkillSource::Builtin, builtin.clone()));
        }

        roots
    }
}

impl SkillCatalog for FsSkillCatalog {
    fn discover(&self, workdir: &str) -> Result<Vec<SkillManifest>> {
        let mut manifests = Vec::new();
        for (source, root) in self.roots(workdir) {
            scan(&root, source, 0, &mut manifests);
        }
        Ok(manifests)
    }

    fn load_body(&self, manifest: &SkillManifest) -> Result<String> {
        std::fs::read_to_string(&manifest.path).map_err(|error| {
            DomainError::adapter(format!("read {}", manifest.path), error)
        })
    }
}

/// Walks a skills root, collecting every `SKILL.md` it can find.
fn scan(dir: &Path, source: SkillSource, depth: usize, out: &mut Vec<SkillManifest>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };

        if meta.is_dir() {
            let manifest = path.join("SKILL.md");
            if manifest.is_file() {
                if let Some(parsed) = parse_manifest(&manifest, source) {
                    out.push(parsed);
                }
            }
            scan(&path, source, depth + 1, out);
        } else if depth == 0
            && path.extension().and_then(|ext| ext.to_str()) == Some("md")
            && path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md")
        {
            // A loose `.md` at the root of a skills directory is a skill in its
            // own right, which is how single-file skills are written.
            if let Some(parsed) = parse_manifest(&path, source) {
                out.push(parsed);
            }
        }
    }
}

fn parse_manifest(path: &Path, source: SkillSource) -> Option<SkillManifest> {
    let text = std::fs::read_to_string(path).ok()?;
    let front = parse_frontmatter(&text)?;

    let directory = path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string());

    // A single-file skill is named by its file, not its parent directory.
    let is_single_file = path.file_name().and_then(|n| n.to_str()) != Some("SKILL.md");
    let fallback_name = if is_single_file {
        path.file_stem().map(|stem| stem.to_string_lossy().to_string())
    } else {
        directory.clone()
    };

    let name = front
        .get("name")
        .cloned()
        .or(fallback_name.clone())
        .unwrap_or_default();
    let description = front.get("description").cloned().unwrap_or_default();

    // The one fatal condition: a skill with no description can never be routed
    // to, so it is skipped rather than listed as unusable.
    let validation = Validation::check(
        &name,
        &description,
        if is_single_file {
            None
        } else {
            directory.as_deref()
        },
    )
    .ok()?;

    let dir = path.parent().unwrap_or(Path::new("."));
    let executable = dir.join("pyproject.toml").is_file();

    Some(SkillManifest {
        import_name: executable.then(|| SkillManifest::import_name_for(&name)),
        name,
        description,
        kind: if executable {
            SkillKind::Executable
        } else {
            SkillKind::Markdown
        },
        source,
        path: path.to_string_lossy().to_string(),
        license: front.get("license").cloned(),
        compatibility: front.get("compatibility").cloned(),
        allowed_tools: front
            .get("allowed-tools")
            .map(|value| {
                value
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default(),
        disable_model_invocation: front
            .get("disable-model-invocation")
            .map(|value| value.trim() == "true")
            .unwrap_or(false),
        warnings: validation.warnings,
    })
}

/// A deliberately small YAML frontmatter reader.
///
/// It handles `key: value`, quoted values and folded blocks — which is the
/// whole of what the Agent Skills specification requires. Pulling in a YAML
/// parser to read six keys would be a dependency with a much larger blast
/// radius than the feature.
fn parse_frontmatter(text: &str) -> Option<BTreeMap<String, String>> {
    let body = text.strip_prefix("---")?;
    let end = body.find("\n---")?;
    let block = &body[..end];

    let mut fields = BTreeMap::new();
    let mut current_key: Option<String> = None;
    let mut folded = String::new();

    for line in block.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let is_continuation = line.starts_with(' ') || line.starts_with('\t');
        if is_continuation {
            if current_key.is_some() {
                if !folded.is_empty() {
                    folded.push(' ');
                }
                folded.push_str(line.trim());
            }
            continue;
        }

        if let Some(key) = current_key.take() {
            fields.insert(key, folded.trim().to_string());
            folded = String::new();
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches(['"', '\'']).to_string();

        if value == "|" || value == ">" || value.is_empty() {
            current_key = Some(key);
        } else {
            fields.insert(key, value);
        }
    }

    if let Some(key) = current_key {
        fields.insert(key, folded.trim().to_string());
    }

    Some(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A catalog that cannot see the machine's real skill directories.
    fn isolated() -> FsSkillCatalog {
        FsSkillCatalog::default().with_home(None)
    }

    fn write_skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    #[test]
    fn frontmatter_reads_simple_and_folded_values() {
        let parsed = parse_frontmatter(
            "---\nname: pdf-tools\ndescription: >\n  Extracts text\n  and tables.\nlicense: MIT\n---\n# body",
        )
        .unwrap();
        assert_eq!(parsed.get("name").unwrap(), "pdf-tools");
        assert_eq!(parsed.get("description").unwrap(), "Extracts text and tables.");
        assert_eq!(parsed.get("license").unwrap(), "MIT");
    }

    #[test]
    fn a_file_without_frontmatter_is_not_a_skill() {
        assert!(parse_frontmatter("# just a document").is_none());
    }

    #[test]
    fn project_skills_are_discovered_and_described() {
        let root = workspace("project");
        let skills = root.join(".claude/skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(
            &skills,
            "pdf-tools",
            "---\nname: pdf-tools\ndescription: Works with PDFs.\n---\n# PDF Tools\n",
        );

        let found = isolated()
            .discover(&root.to_string_lossy())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "pdf-tools");
        assert_eq!(found[0].source, SkillSource::Project);
        assert_eq!(found[0].kind, SkillKind::Markdown);
    }

    #[test]
    fn a_pyproject_marks_a_skill_executable_and_names_its_import() {
        let root = workspace("executable");
        let skills = root.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(
            &skills,
            "web-search",
            "---\nname: web-search\ndescription: Searches the web.\n---\n",
        );
        std::fs::write(skills.join("web-search/pyproject.toml"), "[project]\n").unwrap();

        let found = isolated()
            .discover(&root.to_string_lossy())
            .unwrap();
        assert_eq!(found[0].kind, SkillKind::Executable);
        assert_eq!(found[0].import_name.as_deref(), Some("web_search"));
    }

    #[test]
    fn a_skill_with_no_description_is_skipped_entirely() {
        let root = workspace("nodesc");
        let skills = root.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(&skills, "broken", "---\nname: broken\n---\n# nothing\n");

        assert!(isolated()
            .discover(&root.to_string_lossy())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_name_that_disagrees_with_its_directory_warns_but_loads() {
        let root = workspace("mismatch");
        let skills = root.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(
            &skills,
            "actual-dir",
            "---\nname: other-name\ndescription: Does things.\n---\n",
        );

        let found = isolated()
            .discover(&root.to_string_lossy())
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .warnings
            .iter()
            .any(|warning| warning.contains("does not match")));
    }

    #[test]
    fn explicit_roots_outrank_everything_and_bodies_load_on_demand() {
        let root = workspace("explicit");
        let extra = root.join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        write_skill(
            &extra,
            "special",
            "---\nname: special\ndescription: Special one.\n---\n# Special\nthe body\n",
        );

        let catalog = FsSkillCatalog::new(vec![extra], None).with_home(None);
        let found = catalog.discover(&root.to_string_lossy()).unwrap();
        assert_eq!(found[0].source, SkillSource::Explicit);
        assert!(catalog.load_body(&found[0]).unwrap().contains("the body"));
    }

    #[test]
    fn hidden_skills_are_flagged_rather_than_dropped() {
        let root = workspace("hidden");
        let skills = root.join(".agents/skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(
            &skills,
            "quiet",
            "---\nname: quiet\ndescription: Hidden one.\ndisable-model-invocation: true\n---\n",
        );

        let found = isolated()
            .discover(&root.to_string_lossy())
            .unwrap();
        assert!(found[0].disable_model_invocation);
    }
}
