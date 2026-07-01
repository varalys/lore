//! Read-only mirror of a coding tool's per-project memory store.
//!
//! Coding tools such as Claude Code write per-project "memory" (running notes,
//! next steps, corrections) into their own private stores that other tools
//! cannot see. This module mirrors those files into Lore's `memories` table so
//! any LLM can read them through Lore's MCP server.
//!
//! The mirror is strictly READ-ONLY: it never creates, modifies, or deletes
//! files in the tool's memory folder. It only reflects the current folder
//! state into the database, adding new memories, updating changed ones, and
//! removing memories whose source file no longer exists.
//!
//! Claude Code stores per-project data under `~/.claude/projects/<slug>/` where
//! `<slug>` is the project's absolute path with the path separator replaced by
//! `-` (for example `/Users/me/proj` becomes `-Users-me-proj`). Sessions live
//! in the `sessions/` folder; memory lives in the sibling `memory/` folder as a
//! `MEMORY.md` index plus per-fact markdown files. Each fact file carries YAML
//! frontmatter with `name`, `description`, and `metadata.type`, followed by the
//! fact body.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::storage::{Database, Memory};

/// The source-tool identifier used for Claude Code memories.
pub const CLAUDE_CODE_TOOL: &str = "claude-code";

/// Frontmatter parsed from a memory markdown file.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    /// Short name of the memory.
    #[serde(default)]
    name: Option<String>,

    /// Human-readable description of the memory.
    #[serde(default)]
    description: Option<String>,

    /// Nested metadata block (holds the memory type).
    #[serde(default)]
    metadata: Option<FrontmatterMetadata>,
}

/// The `metadata` block within a memory's frontmatter.
#[derive(Debug, Default, Deserialize)]
struct FrontmatterMetadata {
    /// The memory type (e.g., user, feedback, project, reference).
    #[serde(default, rename = "type")]
    memory_type: Option<String>,
}

/// A memory parsed from a single markdown file on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMemory {
    /// Short name (frontmatter `name` or the file stem).
    pub name: String,

    /// Optional description from frontmatter.
    pub description: Option<String>,

    /// Optional memory type from `metadata.type`.
    pub memory_type: Option<String>,

    /// The memory body following any frontmatter.
    pub content: String,

    /// Absolute path of the source file.
    pub file_path: String,

    /// Source file modification time.
    pub updated_at: DateTime<Utc>,
}

/// Statistics describing what a single refresh changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MirrorStats {
    /// Number of memories added or updated from the folder.
    pub upserted: usize,

    /// Number of memories removed because their source file was gone.
    pub removed: usize,
}

/// Mirrors a coding tool's memory folder into Lore's database.
///
/// The base directory (the tool's per-project root) is injectable so tests can
/// point the mirror at a temporary folder instead of the real `~/.claude`.
pub struct MemoryMirror {
    /// The tool's per-project storage root (e.g., `~/.claude/projects`).
    base_dir: PathBuf,

    /// The source-tool identifier stored on mirrored memories.
    source_tool: String,
}

impl MemoryMirror {
    /// Creates a mirror for Claude Code, reading from `~/.claude/projects`.
    pub fn claude() -> Self {
        Self {
            base_dir: claude_projects_dir(),
            source_tool: CLAUDE_CODE_TOOL.to_string(),
        }
    }

    /// Creates a mirror with an explicit base directory and source tool.
    ///
    /// Intended for tests that point the mirror at a temporary folder instead
    /// of the real `~/.claude`, so tests never touch a developer's real memory
    /// store.
    #[cfg(test)]
    pub fn with_base_dir(base_dir: impl Into<PathBuf>, source_tool: impl Into<String>) -> Self {
        Self {
            base_dir: base_dir.into(),
            source_tool: source_tool.into(),
        }
    }

    /// Resolves the memory folder for a project.
    ///
    /// This is `<base_dir>/<slug>/memory` where `<slug>` is the project's
    /// absolute path with separators replaced by `-`.
    pub fn memory_dir(&self, project_path: &Path) -> PathBuf {
        self.base_dir
            .join(project_slug(project_path))
            .join("memory")
    }

    /// Refreshes the mirror for a project to match the current folder state.
    ///
    /// Adds new memories, updates changed ones, and removes memories whose
    /// source file no longer exists. If the memory folder does not exist the
    /// project simply has no memories and any previously mirrored ones are
    /// removed; this is not an error.
    ///
    /// This is a read-only operation with respect to the tool's memory folder:
    /// it only reads the folder and writes to Lore's database.
    pub fn refresh(&self, db: &Database, project_path: &Path) -> Result<MirrorStats> {
        let project_key = normalized_project_key(project_path);
        let memory_dir = self.memory_dir(project_path);

        let parsed = parse_memory_dir(&memory_dir)?;
        let current_paths: HashSet<String> = parsed.iter().map(|m| m.file_path.clone()).collect();

        let existing = db.get_memories(&project_key, &self.source_tool)?;

        let mut stats = MirrorStats::default();

        // Remove memories whose source file no longer exists.
        for memory in &existing {
            if !current_paths.contains(&memory.file_path) && db.delete_memory(&memory.id)? {
                stats.removed += 1;
            }
        }

        // Add or update the memories reflected on disk.
        for parsed_memory in &parsed {
            let memory = Memory {
                id: Uuid::new_v4(),
                project_path: project_key.clone(),
                source_tool: self.source_tool.clone(),
                name: parsed_memory.name.clone(),
                description: parsed_memory.description.clone(),
                memory_type: parsed_memory.memory_type.clone(),
                content: parsed_memory.content.clone(),
                file_path: parsed_memory.file_path.clone(),
                updated_at: parsed_memory.updated_at,
            };
            db.upsert_memory(&memory)?;
            stats.upserted += 1;
        }

        Ok(stats)
    }
}

/// Returns the path to the Claude Code projects directory (`~/.claude/projects`).
fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}

/// Computes the Claude-style project slug for a path.
///
/// Claude stores per-project data under a directory whose name is the project's
/// absolute path with path separators replaced by `-`. The project key is
/// normalized (trailing separators trimmed) first so a path reported with a
/// trailing slash yields the same slug as one without.
fn project_slug(project_path: &Path) -> String {
    let normalized = normalized_project_key(project_path);
    normalized.replace(['/', '\\'], "-")
}

/// Returns a stable string key for a project path.
///
/// Trailing path separators are trimmed so the same repository always maps to
/// the same key regardless of whether callers include a trailing slash (git
/// working directories, for example, are reported with one).
fn normalized_project_key(project_path: &Path) -> String {
    let raw = project_path.to_string_lossy();
    let trimmed = raw.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        raw.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolves the project path to scope memories to.
///
/// When `explicit` is provided it is used directly; otherwise the current
/// working directory is used. The path is then resolved to its git top-level
/// (working directory) when inside a repository, so memories are consistently
/// scoped to the repository root. Trailing separators are trimmed.
pub fn resolve_project_path(explicit: Option<&str>) -> Result<PathBuf> {
    let base = match explicit {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().context("Failed to determine current directory")?,
    };

    // Prefer the git top-level so memories are scoped to the repository root.
    let resolved = match crate::git::repo_info(&base) {
        Ok(info) if !info.path.is_empty() => PathBuf::from(info.path),
        _ => base,
    };

    Ok(PathBuf::from(normalized_project_key(&resolved)))
}

/// Parses all memory markdown files in a folder.
///
/// Returns an empty vector when the folder does not exist. `MEMORY.md` is
/// captured as an index entry alongside the per-fact files. Files that cannot
/// be read are skipped with a debug log rather than failing the whole refresh.
pub fn parse_memory_dir(memory_dir: &Path) -> Result<Vec<ParsedMemory>> {
    if !memory_dir.exists() {
        return Ok(Vec::new());
    }

    let mut memories = Vec::new();

    for entry in fs::read_dir(memory_dir)
        .with_context(|| format!("Failed to read memory directory {}", memory_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        match path.extension().and_then(|e| e.to_str()) {
            Some("md") => {}
            _ => continue,
        }

        match parse_memory_file(&path) {
            Ok(memory) => memories.push(memory),
            Err(e) => {
                tracing::debug!("Skipping unreadable memory file {}: {}", path.display(), e);
            }
        }
    }

    // Sort for deterministic ordering across platforms.
    memories.sort_by(|a, b| a.file_path.cmp(&b.file_path));

    Ok(memories)
}

/// Parses a single memory markdown file into a [`ParsedMemory`].
fn parse_memory_file(path: &Path) -> Result<ParsedMemory> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read memory file {}", path.display()))?;

    let (frontmatter, body) = split_frontmatter(&raw);

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("memory")
        .to_string();

    let is_index = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("MEMORY.md"))
        .unwrap_or(false);

    let name = frontmatter
        .as_ref()
        .and_then(|f| f.name.clone())
        .unwrap_or(file_stem);

    let description = frontmatter.as_ref().and_then(|f| f.description.clone());

    let memory_type = frontmatter
        .as_ref()
        .and_then(|f| f.metadata.as_ref())
        .and_then(|m| m.memory_type.clone())
        .or_else(|| is_index.then(|| "index".to_string()));

    let updated_at = file_modified_time(path);

    Ok(ParsedMemory {
        name,
        description,
        memory_type,
        content: body,
        file_path: path.to_string_lossy().to_string(),
        updated_at,
    })
}

/// Returns a file's modification time as a UTC timestamp.
///
/// Falls back to the current time when the metadata is unavailable.
fn file_modified_time(path: &Path) -> DateTime<Utc> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now())
}

/// Splits a markdown document into optional YAML frontmatter and its body.
///
/// Frontmatter is a leading block delimited by lines containing only `---`.
/// Returns the parsed frontmatter (when present and valid) and the trimmed
/// body. When there is no frontmatter, or the closing delimiter is missing,
/// the entire document is treated as the body.
fn split_frontmatter(raw: &str) -> (Option<Frontmatter>, String) {
    let raw = raw.trim_start_matches('\u{feff}');

    let mut lines = raw.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, raw.trim().to_string());
    }

    let mut yaml = String::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut found_close = false;

    for line in lines {
        if !found_close && line.trim_end() == "---" {
            found_close = true;
            continue;
        }
        if found_close {
            body_lines.push(line);
        } else {
            yaml.push_str(line);
            yaml.push('\n');
        }
    }

    if !found_close {
        // No closing delimiter: treat the whole document as body.
        return (None, raw.trim().to_string());
    }

    let frontmatter = serde_saphyr::from_str::<Frontmatter>(&yaml).ok();
    (frontmatter, body_lines.join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// Writes a file with the given content, creating parent directories.
    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent dirs");
        }
        let mut file = fs::File::create(path).expect("Failed to create file");
        file.write_all(content.as_bytes())
            .expect("Failed to write file");
    }

    /// Creates an in-memory-style test database in a temp directory.
    fn create_test_db() -> (Database, tempfile::TempDir) {
        let dir = tempdir().expect("Failed to create temp dir");
        let db_path = dir.path().join("test.db");
        let db = Database::open(&db_path).expect("Failed to open test database");
        (db, dir)
    }

    #[test]
    fn test_project_slug_replaces_separators() {
        let slug = project_slug(Path::new("/Users/me/projects/lore"));
        assert_eq!(slug, "-Users-me-projects-lore");
    }

    #[test]
    fn test_project_slug_trims_trailing_slash() {
        let with_slash = project_slug(Path::new("/Users/me/lore/"));
        let without_slash = project_slug(Path::new("/Users/me/lore"));
        assert_eq!(with_slash, without_slash);
    }

    #[test]
    fn test_split_frontmatter_parses_fields() {
        let raw = "---\nname: Prefer tabs\ndescription: Use tabs not spaces\nmetadata:\n  type: user\n---\nThe user prefers tabs.\n";
        let (fm, body) = split_frontmatter(raw);
        let fm = fm.expect("Should parse frontmatter");
        assert_eq!(fm.name.as_deref(), Some("Prefer tabs"));
        assert_eq!(fm.description.as_deref(), Some("Use tabs not spaces"));
        assert_eq!(
            fm.metadata.and_then(|m| m.memory_type).as_deref(),
            Some("user")
        );
        assert_eq!(body, "The user prefers tabs.");
    }

    #[test]
    fn test_split_frontmatter_no_frontmatter() {
        let raw = "Just some notes without frontmatter.";
        let (fm, body) = split_frontmatter(raw);
        assert!(fm.is_none());
        assert_eq!(body, "Just some notes without frontmatter.");
    }

    #[test]
    fn test_split_frontmatter_missing_close_is_body() {
        let raw = "---\nname: broken\nno closing delimiter here";
        let (fm, body) = split_frontmatter(raw);
        assert!(fm.is_none());
        assert!(body.contains("no closing delimiter"));
    }

    #[test]
    fn test_parse_memory_dir_missing_folder_is_empty() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let memories = parse_memory_dir(&missing).expect("Should not error");
        assert!(memories.is_empty());
    }

    #[test]
    fn test_parse_memory_dir_reads_facts_and_index() {
        let dir = tempdir().unwrap();
        let mem_dir = dir.path().join("memory");
        write_file(&mem_dir.join("MEMORY.md"), "# Index\n- fact-1\n");
        write_file(
            &mem_dir.join("fact-1.md"),
            "---\nname: API base URL\ndescription: Where the API lives\nmetadata:\n  type: reference\n---\nThe API base URL is https://example.com.\n",
        );

        let memories = parse_memory_dir(&mem_dir).expect("Should parse");
        assert_eq!(memories.len(), 2);

        let fact = memories
            .iter()
            .find(|m| m.name == "API base URL")
            .expect("Should find fact");
        assert_eq!(fact.description.as_deref(), Some("Where the API lives"));
        assert_eq!(fact.memory_type.as_deref(), Some("reference"));
        assert!(fact.content.contains("https://example.com"));

        let index = memories
            .iter()
            .find(|m| m.name == "MEMORY")
            .expect("Should capture index");
        assert_eq!(index.memory_type.as_deref(), Some("index"));
    }

    #[test]
    fn test_refresh_captures_memories_scoped_to_project() {
        let (db, _db_dir) = create_test_db();
        let base = tempdir().unwrap();
        let project = Path::new("/tmp/example-project");

        let mirror = MemoryMirror::with_base_dir(base.path(), CLAUDE_CODE_TOOL);
        let mem_dir = mirror.memory_dir(project);
        write_file(
            &mem_dir.join("fact-1.md"),
            "---\nname: Fact one\ndescription: First fact\nmetadata:\n  type: project\n---\nBody one.\n",
        );

        let stats = mirror
            .refresh(&db, project)
            .expect("Refresh should succeed");
        assert_eq!(stats.upserted, 1);
        assert_eq!(stats.removed, 0);

        let memories = db
            .get_memories("/tmp/example-project", CLAUDE_CODE_TOOL)
            .expect("Should list memories");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].name, "Fact one");
        assert_eq!(memories[0].memory_type.as_deref(), Some("project"));
        assert_eq!(memories[0].project_path, "/tmp/example-project");

        // Memories are scoped to the project: a different project sees none.
        let other = db
            .get_memories("/tmp/other-project", CLAUDE_CODE_TOOL)
            .expect("Should query");
        assert!(other.is_empty());
    }

    #[test]
    fn test_refresh_removes_deleted_files() {
        let (db, _db_dir) = create_test_db();
        let base = tempdir().unwrap();
        let project = Path::new("/tmp/mirror-remove");

        let mirror = MemoryMirror::with_base_dir(base.path(), CLAUDE_CODE_TOOL);
        let mem_dir = mirror.memory_dir(project);
        let fact_a = mem_dir.join("a.md");
        let fact_b = mem_dir.join("b.md");
        write_file(&fact_a, "---\nname: A\n---\nBody A.\n");
        write_file(&fact_b, "---\nname: B\n---\nBody B.\n");

        mirror.refresh(&db, project).expect("Initial refresh");
        assert_eq!(
            db.get_memories("/tmp/mirror-remove", CLAUDE_CODE_TOOL)
                .unwrap()
                .len(),
            2
        );

        // Remove one source file; the mirror should drop it on refresh.
        fs::remove_file(&fact_b).expect("Failed to remove file");
        let stats = mirror.refresh(&db, project).expect("Second refresh");
        assert_eq!(stats.removed, 1);

        let remaining = db
            .get_memories("/tmp/mirror-remove", CLAUDE_CODE_TOOL)
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "A");
    }

    #[test]
    fn test_refresh_updates_changed_files() {
        let (db, _db_dir) = create_test_db();
        let base = tempdir().unwrap();
        let project = Path::new("/tmp/mirror-update");

        let mirror = MemoryMirror::with_base_dir(base.path(), CLAUDE_CODE_TOOL);
        let mem_dir = mirror.memory_dir(project);
        let fact = mem_dir.join("fact.md");
        write_file(&fact, "---\nname: Original\n---\nOriginal body.\n");

        mirror.refresh(&db, project).expect("Initial refresh");
        let first = db
            .get_memories("/tmp/mirror-update", CLAUDE_CODE_TOOL)
            .unwrap();
        assert_eq!(first.len(), 1);
        let original_id = first[0].id;

        // Rewrite the same file with new content.
        write_file(&fact, "---\nname: Updated\n---\nUpdated body.\n");
        mirror.refresh(&db, project).expect("Second refresh");

        let updated = db
            .get_memories("/tmp/mirror-update", CLAUDE_CODE_TOOL)
            .unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].name, "Updated");
        assert!(updated[0].content.contains("Updated body"));
        // The id is preserved across updates for the same source file.
        assert_eq!(updated[0].id, original_id);
    }

    #[test]
    fn test_refresh_adds_new_files() {
        let (db, _db_dir) = create_test_db();
        let base = tempdir().unwrap();
        let project = Path::new("/tmp/mirror-add");

        let mirror = MemoryMirror::with_base_dir(base.path(), CLAUDE_CODE_TOOL);
        let mem_dir = mirror.memory_dir(project);
        write_file(&mem_dir.join("a.md"), "---\nname: A\n---\nBody A.\n");
        mirror.refresh(&db, project).expect("Initial refresh");

        write_file(&mem_dir.join("b.md"), "---\nname: B\n---\nBody B.\n");
        let stats = mirror.refresh(&db, project).expect("Second refresh");
        assert_eq!(stats.upserted, 2);

        let memories = db
            .get_memories("/tmp/mirror-add", CLAUDE_CODE_TOOL)
            .unwrap();
        assert_eq!(memories.len(), 2);
    }

    #[test]
    fn test_refresh_missing_folder_yields_no_memories() {
        let (db, _db_dir) = create_test_db();
        let base = tempdir().unwrap();
        let project = Path::new("/tmp/mirror-empty");

        let mirror = MemoryMirror::with_base_dir(base.path(), CLAUDE_CODE_TOOL);
        let stats = mirror
            .refresh(&db, project)
            .expect("Refresh should not error on missing folder");
        assert_eq!(stats.upserted, 0);
        assert_eq!(stats.removed, 0);
        assert!(db
            .get_memories("/tmp/mirror-empty", CLAUDE_CODE_TOOL)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_search_memories_returns_matches() {
        let (db, _db_dir) = create_test_db();
        let base = tempdir().unwrap();
        let project = Path::new("/tmp/mirror-search");

        let mirror = MemoryMirror::with_base_dir(base.path(), CLAUDE_CODE_TOOL);
        let mem_dir = mirror.memory_dir(project);
        write_file(
            &mem_dir.join("auth.md"),
            "---\nname: Auth flow\n---\nUse OAuth with PKCE for authentication.\n",
        );
        write_file(
            &mem_dir.join("db.md"),
            "---\nname: Database\n---\nThe project uses SQLite for storage.\n",
        );
        mirror.refresh(&db, project).expect("Refresh");

        let results = db
            .search_memories("/tmp/mirror-search", CLAUDE_CODE_TOOL, "OAuth", 10)
            .expect("Search should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Auth flow");

        let none = db
            .search_memories("/tmp/mirror-search", CLAUDE_CODE_TOOL, "kubernetes", 10)
            .expect("Search should succeed");
        assert!(none.is_empty());
    }
}
