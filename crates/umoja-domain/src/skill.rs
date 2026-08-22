//! Skill discovery, validation and precedence, per the Agent Skills standard.
//!
//! The standard is applied the way prime-agent applies it: warn loudly, load
//! anyway. A skill with a bad name is still a skill somebody wrote; refusing it
//! outright helps nobody. The single hard failure is a missing description,
//! because the description is what decides when the skill loads — without it
//! there is nothing to route on.

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillKind {
    /// `SKILL.md` and whatever it references.
    Markdown,
    /// Also ships a package the kernel can import and call.
    Executable,
}

/// Where a skill came from. The order of this enum *is* the precedence order:
/// earlier wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Named explicitly on the command line. Always wins.
    Explicit,
    /// `.prime/agent/skills`, `.agents/skills`, `.claude/skills` in the project.
    Project,
    /// `~/.prime/agent/skills`, `~/.agents/skills`, `~/.claude/skills`.
    Personal,
    /// Discovered inside a dependency.
    Package,
    /// Shipped with this binary. Lowest, so anything a user writes overrides it.
    Builtin,
}

impl SkillSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Project => "project",
            Self::Personal => "personal",
            Self::Package => "package",
            Self::Builtin => "builtin",
        }
    }
}

/// A parsed `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub kind: SkillKind,
    pub source: SkillSource,
    pub path: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Hidden from the startup list; reachable only by explicit invocation.
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// The import name for an executable skill: hyphens become underscores.
    pub import_name: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl SkillManifest {
    pub const MAX_NAME: usize = 64;
    pub const MAX_DESCRIPTION: usize = 1024;
    pub const MAX_COMPATIBILITY: usize = 500;

    /// The XML line placed in a startup prompt. Only this — never the body —
    /// so a hundred installed skills cost a hundred lines, not a hundred files.
    pub fn prompt_line(&self) -> String {
        format!(
            "<skill name=\"{}\" source=\"{}\">{}</skill>",
            self.name,
            self.source.label(),
            self.description
        )
    }

    pub fn import_name_for(name: &str) -> String {
        name.replace('-', "_")
    }
}

/// The result of checking a manifest: what is wrong, and whether it is fatal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validation {
    pub warnings: Vec<String>,
}

impl Validation {
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }

    /// Validates name and description against the standard.
    ///
    /// Returns `Err` only for a missing description — the one condition that
    /// makes a skill unusable rather than merely non-conforming.
    pub fn check(name: &str, description: &str, directory: Option<&str>) -> Result<Self> {
        let description = description.trim();
        if description.is_empty() {
            return Err(DomainError::invalid(
                "a skill needs a description; it is what decides when the skill loads",
            ));
        }

        let mut warnings = Vec::new();

        if description.chars().count() > SkillManifest::MAX_DESCRIPTION {
            warnings.push(format!(
                "description is {} characters; the standard allows {}",
                description.chars().count(),
                SkillManifest::MAX_DESCRIPTION
            ));
        }

        if name.is_empty() {
            warnings.push("skill has no name".into());
        }
        if name.chars().count() > SkillManifest::MAX_NAME {
            warnings.push(format!(
                "name is {} characters; the standard allows {}",
                name.chars().count(),
                SkillManifest::MAX_NAME
            ));
        }
        if name
            .chars()
            .any(|ch| !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'))
        {
            warnings.push(format!(
                "name '{name}' should use only lowercase letters, digits and hyphens"
            ));
        }
        if name.starts_with('-') || name.ends_with('-') {
            warnings.push(format!("name '{name}' should not start or end with a hyphen"));
        }
        if name.contains("--") {
            warnings.push(format!("name '{name}' has consecutive hyphens"));
        }
        if let Some(dir) = directory {
            if !dir.is_empty() && dir != name {
                warnings.push(format!(
                    "name '{name}' does not match its directory '{dir}'"
                ));
            }
        }

        Ok(Self { warnings })
    }
}

/// Resolves name collisions across sources.
///
/// Same source, same name: the first one found wins and the loser is reported,
/// matching prime-agent. Different sources: the higher-precedence source wins,
/// silently, because that is the whole point of having precedence.
pub fn resolve_collisions(mut manifests: Vec<SkillManifest>) -> (Vec<SkillManifest>, Vec<String>) {
    manifests.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.name.cmp(&b.name)));

    let mut kept: Vec<SkillManifest> = Vec::new();
    let mut notes = Vec::new();

    for manifest in manifests {
        match kept.iter().find(|existing| existing.name == manifest.name) {
            Some(winner) if winner.source == manifest.source => {
                notes.push(format!(
                    "two '{}' skills in {} scope; kept {}, ignored {}",
                    manifest.name,
                    manifest.source.label(),
                    winner.path,
                    manifest.path
                ));
            }
            Some(winner) => {
                notes.push(format!(
                    "'{}' from {} overrides the {} one at {}",
                    manifest.name,
                    winner.source.label(),
                    manifest.source.label(),
                    manifest.path
                ));
            }
            None => kept.push(manifest),
        }
    }

    kept.sort_by(|a, b| a.name.cmp(&b.name));
    (kept, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, source: SkillSource, path: &str) -> SkillManifest {
        SkillManifest {
            name: name.into(),
            description: "does a thing".into(),
            kind: SkillKind::Markdown,
            source,
            path: path.into(),
            license: None,
            compatibility: None,
            allowed_tools: Vec::new(),
            disable_model_invocation: false,
            import_name: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_missing_description_is_the_only_fatal_error() {
        assert!(Validation::check("good-name", "  ", None).is_err());
        assert!(Validation::check("BAD_NAME", "still described", None).is_ok());
    }

    #[test]
    fn nonconforming_names_warn_but_load() {
        let validation = Validation::check("PDF--Processing-", "d", Some("pdf-processing")).unwrap();
        assert!(!validation.is_clean());
        assert_eq!(validation.warnings.len(), 4);
    }

    #[test]
    fn a_conforming_skill_produces_no_warnings() {
        let validation = Validation::check("pdf-processing", "d", Some("pdf-processing")).unwrap();
        assert!(validation.is_clean());
    }

    #[test]
    fn user_skills_override_builtins() {
        let (kept, notes) = resolve_collisions(vec![
            manifest("websearch", SkillSource::Builtin, "/builtin/websearch"),
            manifest("websearch", SkillSource::Personal, "/home/me/websearch"),
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].source, SkillSource::Personal);
        assert!(notes[0].contains("overrides"));
    }

    #[test]
    fn same_scope_collisions_keep_the_first_and_say_so() {
        let (kept, notes) = resolve_collisions(vec![
            manifest("dup", SkillSource::Project, "/a/dup"),
            manifest("dup", SkillSource::Project, "/b/dup"),
        ]);
        assert_eq!(kept.len(), 1);
        assert!(notes[0].contains("ignored"));
    }

    #[test]
    fn import_names_underscore_hyphens() {
        assert_eq!(SkillManifest::import_name_for("web-search"), "web_search");
    }
}
