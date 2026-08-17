//! Skill discovery and progressive disclosure.
//!
//! Only descriptions go into the startup prompt; bodies load when a task
//! matches. That is the entire economics of the skills system — a hundred
//! installed skills should cost a hundred lines of context, not a hundred
//! files.

use std::sync::Arc;

use pa_domain::prelude::*;
use pa_domain::skill::resolve_collisions;

use crate::Env;

pub struct SkillService {
    catalog: Arc<dyn SkillCatalog>,
}

impl std::fmt::Debug for SkillService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SkillService")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillIndex {
    pub skills: Vec<SkillManifest>,
    /// Collisions and standard violations, reported rather than enforced.
    pub notes: Vec<String>,
}

impl SkillService {
    pub fn new(_env: Env, catalog: Arc<dyn SkillCatalog>) -> Self {
        Self { catalog }
    }

    pub fn index(&self, workdir: &str) -> Result<SkillIndex> {
        let discovered = self.catalog.discover(workdir)?;
        let (skills, mut notes) = resolve_collisions(discovered);
        for skill in &skills {
            for warning in &skill.warnings {
                notes.push(format!("{}: {warning}", skill.name));
            }
        }
        Ok(SkillIndex { skills, notes })
    }

    pub fn get(&self, workdir: &str, name: &str) -> Result<SkillManifest> {
        self.index(workdir)?
            .skills
            .into_iter()
            .find(|skill| skill.name == name)
            .ok_or_else(|| DomainError::not_found("skill", name))
    }

    /// The full instructions, loaded only when something actually matched.
    pub fn body(&self, workdir: &str, name: &str) -> Result<String> {
        let manifest = self.get(workdir, name)?;
        self.catalog.load_body(&manifest)
    }

    /// The startup block: one line per visible skill.
    ///
    /// Skills marked `disable-model-invocation` are omitted — they exist, but
    /// only an explicit call reaches them.
    pub fn prompt_block(&self, workdir: &str) -> Result<String> {
        let index = self.index(workdir)?;
        let visible: Vec<&SkillManifest> = index
            .skills
            .iter()
            .filter(|skill| !skill.disable_model_invocation)
            .collect();

        if visible.is_empty() {
            return Ok(String::new());
        }

        let mut out = String::from("<skills>\n");
        for skill in visible {
            out.push_str("  ");
            out.push_str(&skill.prompt_line());
            out.push('\n');
        }
        out.push_str("</skills>");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;

    fn manifest(name: &str, source: SkillSource) -> SkillManifest {
        SkillManifest {
            name: name.into(),
            description: format!("{name} does a thing"),
            kind: SkillKind::Markdown,
            source,
            path: format!("/{}/{name}", source.label()),
            license: None,
            compatibility: None,
            allowed_tools: Vec::new(),
            disable_model_invocation: false,
            import_name: None,
            warnings: Vec::new(),
        }
    }

    fn fixture(manifests: Vec<SkillManifest>) -> SkillService {
        let (env, _clock) = env();
        let catalog = Arc::new(MemSkills::default());
        *catalog.manifests.lock().unwrap() = manifests;
        SkillService::new(env, catalog)
    }

    #[test]
    fn a_personal_skill_overrides_a_builtin_of_the_same_name() {
        let service = fixture(vec![
            manifest("websearch", SkillSource::Builtin),
            manifest("websearch", SkillSource::Personal),
        ]);
        let index = service.index("/work").unwrap();
        assert_eq!(index.skills.len(), 1);
        assert_eq!(index.skills[0].source, SkillSource::Personal);
        assert!(index.notes[0].contains("overrides"));
    }

    #[test]
    fn the_prompt_block_carries_descriptions_not_bodies() {
        let service = fixture(vec![manifest("pdf-tools", SkillSource::Project)]);
        let block = service.prompt_block("/work").unwrap();
        assert!(block.contains("pdf-tools does a thing"));
        assert!(!block.contains("body"));
    }

    #[test]
    fn hidden_skills_stay_out_of_the_prompt_but_remain_callable() {
        let mut hidden = manifest("secret-tool", SkillSource::Project);
        hidden.disable_model_invocation = true;
        let service = fixture(vec![hidden, manifest("visible", SkillSource::Project)]);

        let block = service.prompt_block("/work").unwrap();
        assert!(!block.contains("secret-tool"));
        assert!(block.contains("visible"));
        assert!(service.get("/work", "secret-tool").is_ok());
    }

    #[test]
    fn bodies_load_only_when_asked_for() {
        let service = fixture(vec![manifest("pdf-tools", SkillSource::Project)]);
        assert!(service.body("/work", "pdf-tools").unwrap().contains("body"));
        assert!(service.body("/work", "nope").is_err());
    }

    #[test]
    fn warnings_are_surfaced_without_dropping_the_skill() {
        let mut sloppy = manifest("Bad_Name", SkillSource::Project);
        sloppy.warnings = vec!["name should use lowercase".into()];
        let service = fixture(vec![sloppy]);

        let index = service.index("/work").unwrap();
        assert_eq!(index.skills.len(), 1);
        assert!(index.notes.iter().any(|note| note.contains("lowercase")));
    }

    #[test]
    fn an_empty_catalog_renders_nothing() {
        assert!(fixture(vec![]).prompt_block("/work").unwrap().is_empty());
    }
}
