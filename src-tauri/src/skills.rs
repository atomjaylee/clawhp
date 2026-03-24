//! Skills discovery, marketplace, and installation.

use crate::config::{parse_json_value_from_output, read_openclaw_config, run_openclaw_args_timeout};
use crate::types::CommandResult;
use crate::util::command::run_cmd_owned_timeout;
use crate::util::path::{
    command_exists, collect_openclaw_install_paths, get_openclaw_home,
    refresh_path, resolve_openclaw_binary_path,
};
use reqwest::Url;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub path: String,
    pub enabled: bool,
    #[serde(rename = "originRegistry", skip_serializing_if = "Option::is_none")]
    pub origin_registry: Option<String>,
    #[serde(rename = "originSlug", skip_serializing_if = "Option::is_none")]
    pub origin_slug: Option<String>,
    #[serde(rename = "installedVersion", skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillRequirementState {
    #[serde(default)]
    pub bins: Vec<String>,
    #[serde(default)]
    pub any_bins: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub config: Vec<String>,
    #[serde(default)]
    pub os: Vec<String>,
}

impl SkillRequirementState {
    fn is_empty(&self) -> bool {
        self.bins.is_empty()
            && self.any_bins.is_empty()
            && self.env.is_empty()
            && self.config.is_empty()
            && self.os.is_empty()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SkillInstallHint {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub bins: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawSkillInfo {
    pub name: String,
    pub dir_name: String,
    pub description: String,
    pub emoji: Option<String>,
    pub eligible: bool,
    pub disabled: bool,
    pub blocked_by_allowlist: bool,
    pub source: String,
    pub bundled: bool,
    pub homepage: Option<String>,
    pub missing: SkillRequirementState,
    pub install_hints: Vec<SkillInstallHint>,
    pub managed_installed: bool,
    pub managed_version: Option<String>,
    pub managed_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillsDashboardSummary {
    pub managed_count: usize,
    pub bundled_count: usize,
    pub workspace_count: usize,
    pub eligible_count: usize,
    pub missing_requirement_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillsDashboardSnapshot {
    pub workspace_dir: String,
    pub managed_skills_dir: String,
    pub managed_skills: Vec<SkillInfo>,
    pub openclaw_skills: Vec<OpenClawSkillInfo>,
    pub summary: SkillsDashboardSummary,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillsRequirementSnapshot {
    pub eligible_count: usize,
    pub missing_requirement_count: usize,
    #[serde(default)]
    pub install_hints_by_skill: BTreeMap<String, Vec<SkillInstallHint>>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillMarketplaceEntry {
    pub slug: String,
    pub display_name: String,
    pub summary: String,
    pub version: Option<String>,
    pub updated_at: Option<i64>,
    pub marketplace: String,
    pub marketplace_label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct SkillOriginRecord {
    registry: Option<String>,
    slug: Option<String>,
    installed_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenClawSkillsCheckResponse {
    summary: OpenClawSkillsCheckSummary,
    #[serde(default)]
    missing_requirements: Vec<OpenClawSkillMissingRequirementRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenClawSkillsCheckSummary {
    eligible: usize,
    missing_requirements: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenClawSkillMissingRequirementRecord {
    name: String,
    #[serde(default)]
    install: Vec<SkillInstallHint>,
}

#[derive(Clone, Copy)]
struct SkillMarketplacePreset {
    id: &'static str,
    label: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TencentSkillHubResponse {
    code: i32,
    #[serde(default)]
    data: TencentSkillHubPayload,
    message: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TencentSkillHubPayload {
    #[serde(default)]
    skills: Vec<TencentSkillHubSkillRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TencentSkillHubSkillRecord {
    slug: Option<String>,
    name: Option<String>,
    description: Option<String>,
    description_zh: Option<String>,
    version: Option<String>,
    updated_at: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SkillMarkdownMetadataRecord {
    openclaw: Option<SkillMarkdownOpenClawMetadataRecord>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SkillMarkdownOpenClawMetadataRecord {
    emoji: Option<String>,
    #[serde(default)]
    os: Vec<String>,
    #[serde(default)]
    requires: SkillRequirementState,
    #[serde(default)]
    install: Vec<SkillInstallRecipe>,
}

#[derive(Debug, Default)]
struct SkillMarkdownSummary {
    name: String,
    description: String,
    homepage: Option<String>,
    metadata: SkillMarkdownOpenClawMetadataRecord,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct SkillInstallRecipe {
    id: String,
    kind: String,
    label: String,
    #[allow(dead_code)]
    #[serde(default)]
    bins: Vec<String>,
    formula: Option<String>,
    package: Option<String>,
    module: Option<String>,
    url: Option<String>,
    archive: Option<String>,
    extract: Option<bool>,
    strip_components: Option<u32>,
    target_dir: Option<String>,
    #[serde(default)]
    os: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(dead_code)] // Reserved for future install-status API
pub struct SkillInstallStatus {
    pub id: String,
    pub name: String,
    pub status: String, // pending | installing | completed | failed
    pub error: Option<String>,
}

const TENCENT_SKILLHUB_SITE_URL: &str = "https://skillhub.tencent.com";
const TENCENT_SKILLHUB_API_LIST_URL: &str = "https://lightmake.site/api/skills";
const TENCENT_SKILLHUB_API_TOP_URL: &str = "https://lightmake.site/api/skills/top";
const TENCENT_SKILLHUB_INDEX_URL: &str =
    "https://skillhub-1388575217.cos.ap-guangzhou.myqcloud.com/skills.json";
const TENCENT_SKILLHUB_INSTALL_SCRIPT_URL: &str =
    "https://skillhub-1388575217.cos.ap-guangzhou.myqcloud.com/install/install.sh";
const TENCENT_SKILLHUB_PRIMARY_DOWNLOAD_URL_TEMPLATE: &str =
    "https://lightmake.site/api/v1/download?slug={slug}";

fn current_openclaw_os_tag() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

fn strip_json_like_trailing_commas(input: &str) -> String {
    let chars = input.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in chars.iter().enumerate() {
        if in_string {
            output.push(*ch);
            if escaped {
                escaped = false;
            } else if *ch == '\\' {
                escaped = true;
            } else if *ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                output.push(*ch);
            }
            ',' => {
                let mut lookahead = index + 1;
                while lookahead < chars.len() && chars[lookahead].is_whitespace() {
                    lookahead += 1;
                }
                if lookahead < chars.len() && matches!(chars[lookahead], '}' | ']') {
                    continue;
                }
                output.push(*ch);
            }
            _ => output.push(*ch),
        }
    }

    output
}

fn extract_braced_json_after_marker(content: &str, marker: &str) -> Option<String> {
    let marker_index = content.find(marker)?;
    let after_marker = &content[marker_index + marker.len()..];
    let brace_index = after_marker.find('{')?;
    let slice = &after_marker[brace_index..];

    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in slice.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(slice[..=index].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

fn extract_frontmatter_block(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }

    let mut block = String::new();
    for line in lines {
        if line.trim() == "---" {
            return Some(block);
        }
        if !block.is_empty() {
            block.push('\n');
        }
        block.push_str(line);
    }

    None
}

fn trim_frontmatter_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn extract_frontmatter_scalar(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    let frontmatter = extract_frontmatter_block(content)?;

    frontmatter
        .lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix(&prefix)
                .map(trim_frontmatter_scalar)
                .filter(|value| !value.is_empty())
        })
}

fn parse_skill_markdown_metadata_from_content(
    content: &str,
) -> Result<SkillMarkdownMetadataRecord, String> {
    let raw_metadata = extract_braced_json_after_marker(content, "metadata:")
        .ok_or_else(|| "metadata 不存在或格式不受支持".to_string())?;
    let normalized = strip_json_like_trailing_commas(&raw_metadata);
    serde_json::from_str::<SkillMarkdownMetadataRecord>(&normalized)
        .map_err(|error| format!("metadata 解析失败: {}", error))
}

fn parse_skill_markdown_summary(
    skill_path: &Path,
    fallback_name: &str,
) -> Result<SkillMarkdownSummary, String> {
    let content = std::fs::read_to_string(skill_path)
        .map_err(|error| format!("无法读取 {}: {}", skill_path.display(), error))?;
    let metadata = parse_skill_markdown_metadata_from_content(&content)
        .ok()
        .and_then(|record| record.openclaw)
        .unwrap_or_default();

    Ok(SkillMarkdownSummary {
        name: extract_frontmatter_scalar(&content, "name")
            .unwrap_or_else(|| fallback_name.to_string()),
        description: extract_frontmatter_scalar(&content, "description").unwrap_or_default(),
        homepage: extract_frontmatter_scalar(&content, "homepage"),
        metadata,
    })
}

fn resolve_openclaw_skills_dir(source: &str) -> Option<PathBuf> {
    match source {
        "openclaw-bundled" => resolve_openclaw_package_root().map(|root| root.join("skills")),
        "openclaw-workspace" => Some(
            PathBuf::from(get_openclaw_home())
                .join("workspace")
                .join("skills"),
        ),
        _ => None,
    }
    .filter(|path| path.is_dir())
}

fn normalize_skill_identity(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn build_managed_skill_lookup(managed_skills: &[SkillInfo]) -> BTreeMap<String, SkillInfo> {
    let mut lookup = BTreeMap::new();

    for skill in managed_skills {
        let name_key = normalize_skill_identity(&skill.name);
        if !name_key.is_empty() {
            lookup.entry(name_key).or_insert_with(|| skill.clone());
        }

        if let Some(origin_slug) = skill.origin_slug.as_deref() {
            let slug_key = normalize_skill_identity(origin_slug);
            if !slug_key.is_empty() {
                lookup.entry(slug_key).or_insert_with(|| skill.clone());
            }
        }
    }

    lookup
}

fn config_path_satisfied(config: &serde_json::Value, dotted_path: &str) -> bool {
    let mut current = config;

    for segment in dotted_path.split('.').map(str::trim).filter(|value| !value.is_empty()) {
        let Some(next) = current.get(segment) else {
            return false;
        };
        current = next;
    }

    match current {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(_) => true,
        serde_json::Value::String(text) => !text.trim().is_empty(),
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(entries) => !entries.is_empty(),
    }
}

fn current_os_supported(allowed_os: &[String]) -> bool {
    allowed_os.is_empty()
        || allowed_os
            .iter()
            .any(|value| value.eq_ignore_ascii_case(current_openclaw_os_tag()))
}

fn build_skill_requirement_state(
    config: &serde_json::Value,
    metadata: &SkillMarkdownOpenClawMetadataRecord,
) -> SkillRequirementState {
    let mut missing = SkillRequirementState::default();

    if !current_os_supported(&metadata.os) {
        missing.os = metadata.os.clone();
    }

    for bin in &metadata.requires.bins {
        if !command_exists(bin) {
            missing.bins.push(bin.clone());
        }
    }

    if !metadata.requires.any_bins.is_empty()
        && !metadata
            .requires
            .any_bins
            .iter()
            .any(|bin| command_exists(bin))
    {
        missing.any_bins = metadata.requires.any_bins.clone();
    }

    for env_name in &metadata.requires.env {
        let present = std::env::var(env_name)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if !present {
            missing.env.push(env_name.clone());
        }
    }

    for config_key in &metadata.requires.config {
        if !config_path_satisfied(config, config_key) {
            missing.config.push(config_key.clone());
        }
    }

    missing
}

fn collect_skill_install_hints(recipes: &[SkillInstallRecipe]) -> Vec<SkillInstallHint> {
    recipes
        .iter()
        .filter(|recipe| current_os_supported(&recipe.os))
        .map(|recipe| SkillInstallHint {
            id: recipe.id.clone(),
            kind: recipe.kind.clone(),
            label: recipe.label.clone(),
            bins: recipe.bins.clone(),
        })
        .collect()
}

fn collect_openclaw_skills_from_dir(
    dir: &Path,
    source: &str,
    managed_lookup: &BTreeMap<String, SkillInfo>,
    config: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> Vec<OpenClawSkillInfo> {
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut skills = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        warnings.push(format!("无法读取 Skills 目录 {}", dir.display()));
        return skills;
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        let skill_path = entry_path.join("SKILL.md");
        if !skill_path.is_file() {
            continue;
        }

        let summary = match parse_skill_markdown_summary(&skill_path, &dir_name) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(error);
                continue;
            }
        };
        let missing = build_skill_requirement_state(config, &summary.metadata);
        let display_key = normalize_skill_identity(&summary.name);
        let dir_key = normalize_skill_identity(&dir_name);
        let managed_skill = managed_lookup
            .get(&display_key)
            .or_else(|| managed_lookup.get(&dir_key));

        skills.push(OpenClawSkillInfo {
            name: summary.name,
            dir_name: dir_name.clone(),
            description: summary.description,
            emoji: summary.metadata.emoji.clone(),
            eligible: missing.is_empty(),
            disabled: false,
            blocked_by_allowlist: false,
            source: source.to_string(),
            bundled: source == "openclaw-bundled",
            homepage: summary.homepage,
            missing,
            install_hints: collect_skill_install_hints(&summary.metadata.install),
            managed_installed: managed_skill.is_some(),
            managed_version: managed_skill.and_then(|skill| {
                skill.installed_version.clone().or_else(|| {
                    (skill.version != "unknown").then(|| skill.version.clone())
                })
            }),
            managed_path: managed_skill.map(|skill| skill.path.clone()),
        });
    }

    skills
}

fn parse_skill_install_recipes_from_markdown(
    skill_path: &Path,
) -> Result<Vec<SkillInstallRecipe>, String> {
    let content = std::fs::read_to_string(skill_path)
        .map_err(|error| format!("无法读取 {}: {}", skill_path.display(), error))?;
    let metadata = parse_skill_markdown_metadata_from_content(&content)
        .map_err(|error| format!("解析 {} metadata 失败: {}", skill_path.display(), error))?;

    Ok(metadata
        .openclaw
        .map(|record| record.install)
        .unwrap_or_default())
}

fn find_openclaw_package_root_from_path(path: &Path) -> Option<PathBuf> {
    let candidate = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    for ancestor in candidate.ancestors() {
        if ancestor.join("skills").is_dir()
            && (ancestor.join("package.json").is_file() || ancestor.join("openclaw.mjs").is_file())
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn resolve_openclaw_package_root() -> Option<PathBuf> {
    if let Some(path) = resolve_openclaw_binary_path() {
        if let Some(root) = find_openclaw_package_root_from_path(&path) {
            return Some(root);
        }
    }

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();

    collect_openclaw_install_paths(&home)
        .into_iter()
        .find(|path| {
            path.is_dir()
                && path.join("skills").is_dir()
                && (path.join("package.json").is_file() || path.join("openclaw.mjs").is_file())
        })
}

fn resolve_skill_markdown_candidates(skill_name: &str, source: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let normalized_source = source.trim();

    if normalized_source.is_empty() || normalized_source == "openclaw-bundled" {
        if let Some(package_root) = resolve_openclaw_package_root() {
            candidates.push(
                package_root
                    .join("skills")
                    .join(skill_name)
                    .join("SKILL.md"),
            );
        }
    }

    if normalized_source.is_empty() || normalized_source == "openclaw-workspace" {
        candidates.push(
            PathBuf::from(get_openclaw_home())
                .join("workspace")
                .join("skills")
                .join(skill_name)
                .join("SKILL.md"),
        );
    }

    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|path| seen.insert(path.to_string_lossy().to_string()))
        .collect()
}

fn load_skill_install_recipe(
    skill_name: &str,
    source: &str,
    install_id: &str,
) -> Result<SkillInstallRecipe, String> {
    let mut parse_errors = Vec::new();
    let os_tag = current_openclaw_os_tag();

    for candidate in resolve_skill_markdown_candidates(skill_name, source) {
        if !candidate.is_file() {
            continue;
        }

        match parse_skill_install_recipes_from_markdown(&candidate) {
            Ok(recipes) => {
                if let Some(recipe) = recipes
                    .into_iter()
                    .filter(|recipe| {
                        recipe.os.is_empty() || recipe.os.iter().any(|value| value == os_tag)
                    })
                    .find(|recipe| recipe.id == install_id)
                {
                    return Ok(recipe);
                }
            }
            Err(error) => parse_errors.push(error),
        }
    }

    if !parse_errors.is_empty() {
        return Err(parse_errors.join("；"));
    }

    Err(format!(
        "没有找到 Skill '{}' 的安装信息 '{}'",
        skill_name, install_id
    ))
}

fn invalid_skill_install_input(message: &str) -> CommandResult {
    CommandResult {
        success: false,
        stdout: String::new(),
        stderr: message.to_string(),
        code: Some(1),
    }
}

fn detect_download_filename(url: &str, fallback_name: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|value| {
            value
                .path_segments()
                .and_then(|segments| segments.last().map(|segment| segment.to_string()))
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback_name.to_string())
}

fn install_skill_download_recipe(skill_name: &str, recipe: &SkillInstallRecipe) -> CommandResult {
    let Some(url) = recipe
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return invalid_skill_install_input("下载型依赖缺少 url");
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_root = std::env::temp_dir().join(format!(
        "openclaw-skill-download-{}-{}",
        skill_name, timestamp
    ));
    if let Err(error) = std::fs::create_dir_all(&temp_root) {
        return invalid_skill_install_input(&format!("无法创建临时目录: {}", error));
    }

    let download_name = detect_download_filename(url, &format!("{}.download", recipe.id));
    let downloaded_file = temp_root.join(download_name);
    let curl_result = run_cmd_owned_timeout(
        "curl",
        &[
            "-L".to_string(),
            "--fail".to_string(),
            "--output".to_string(),
            downloaded_file.to_string_lossy().to_string(),
            url.to_string(),
        ],
        Duration::from_secs(300),
    );
    if !curl_result.success {
        let _ = std::fs::remove_dir_all(&temp_root);
        return curl_result;
    }

    let target_subdir = recipe
        .target_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("downloads");
    let target_root = PathBuf::from(get_openclaw_home())
        .join("tools")
        .join(skill_name)
        .join(target_subdir);
    if let Err(error) = std::fs::create_dir_all(&target_root) {
        let _ = std::fs::remove_dir_all(&temp_root);
        return invalid_skill_install_input(&format!("无法创建目标目录: {}", error));
    }

    let mut result = if recipe.extract.unwrap_or(false) {
        let archive_name = recipe
            .archive
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if archive_name.contains("zip") {
            if recipe.strip_components.unwrap_or(0) > 0 {
                let _ = std::fs::remove_dir_all(&temp_root);
                return invalid_skill_install_input("zip 解压暂不支持 stripComponents");
            }
            run_cmd_owned_timeout(
                "unzip",
                &[
                    "-o".to_string(),
                    downloaded_file.to_string_lossy().to_string(),
                    "-d".to_string(),
                    target_root.to_string_lossy().to_string(),
                ],
                Duration::from_secs(300),
            )
        } else {
            let mut args = if archive_name.contains("tar.bz2")
                || archive_name.contains("tbz2")
                || archive_name.contains("tar.xz")
                || archive_name.contains("txz")
                || archive_name.contains("tar.gz")
                || archive_name.contains("tgz")
            {
                let mode = if archive_name.contains("tar.bz2") || archive_name.contains("tbz2") {
                    "-xjf"
                } else if archive_name.contains("tar.xz") || archive_name.contains("txz") {
                    "-xJf"
                } else {
                    "-xzf"
                };
                vec![
                    mode.to_string(),
                    downloaded_file.to_string_lossy().to_string(),
                    "-C".to_string(),
                    target_root.to_string_lossy().to_string(),
                ]
            } else {
                let _ = std::fs::remove_dir_all(&temp_root);
                return invalid_skill_install_input("暂不支持这种下载归档格式");
            };

            if let Some(components) = recipe.strip_components {
                args.push("--strip-components".to_string());
                args.push(components.to_string());
            }

            run_cmd_owned_timeout("tar", &args, Duration::from_secs(300))
        }
    } else {
        let destination = target_root.join(
            downloaded_file
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("download.bin"),
        );
        match std::fs::copy(&downloaded_file, &destination) {
            Ok(_) => CommandResult {
                success: true,
                stdout: format!("已下载到 {}", destination.display()),
                stderr: String::new(),
                code: Some(0),
            },
            Err(error) => invalid_skill_install_input(&format!("写入下载文件失败: {}", error)),
        }
    };

    let _ = std::fs::remove_dir_all(&temp_root);
    if result.success && result.stdout.trim().is_empty() {
        result.stdout = format!("已安装 {}", recipe.label);
    }
    result
}

fn execute_skill_install_recipe(skill_name: &str, recipe: &SkillInstallRecipe) -> CommandResult {
    let kind = recipe.kind.trim().to_ascii_lowercase();
    let result = match kind.as_str() {
        "brew" => {
            let Some(formula) = recipe
                .formula
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return invalid_skill_install_input("brew 安装缺少 formula");
            };
            let mut args = vec!["install".to_string()];
            if recipe.id.contains("cask") || recipe.label.to_ascii_lowercase().contains("cask") {
                args.push("--cask".to_string());
            }
            args.push(formula.to_string());
            run_cmd_owned_timeout("brew", &args, Duration::from_secs(300))
        }
        "go" => {
            let Some(module) = recipe
                .module
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return invalid_skill_install_input("go 安装缺少 module");
            };
            run_cmd_owned_timeout(
                "go",
                &["install".to_string(), module.to_string()],
                Duration::from_secs(300),
            )
        }
        "node" => {
            let Some(package) = recipe
                .package
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return invalid_skill_install_input("node 安装缺少 package");
            };
            run_cmd_owned_timeout(
                "npm",
                &["install".to_string(), "-g".to_string(), package.to_string()],
                Duration::from_secs(300),
            )
        }
        "uv" => {
            let Some(package) = recipe
                .package
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return invalid_skill_install_input("uv 安装缺少 package");
            };
            run_cmd_owned_timeout(
                "uv",
                &[
                    "tool".to_string(),
                    "install".to_string(),
                    "--force".to_string(),
                    package.to_string(),
                ],
                Duration::from_secs(300),
            )
        }
        "download" => install_skill_download_recipe(skill_name, recipe),
        _ => invalid_skill_install_input(&format!("暂不支持 {} 类型的一键安装", recipe.kind)),
    };

    if result.success {
        refresh_path();
    }
    result
}

fn parse_command_json<T: DeserializeOwned>(
    result: CommandResult,
    label: &str,
) -> Result<T, String> {
    if !result.success {
        let detail = if result.stderr.trim().is_empty() {
            result.stdout.trim().to_string()
        } else {
            result.stderr.trim().to_string()
        };
        return Err(if detail.is_empty() {
            label.to_string()
        } else {
            format!("{}: {}", label, detail)
        });
    }

    let stdout = result.stdout.trim();
    if stdout.is_empty() {
        return Err(format!("{}: 返回为空", label));
    }

    let payload = parse_json_value_from_output(stdout)
        .ok_or_else(|| format!("{}: 返回内容不是合法 JSON", label))?;
    serde_json::from_value(payload).map_err(|error| format!("{}: {}", label, error))
}

fn read_skill_origin(entry_path: &Path) -> Option<SkillOriginRecord> {
    let origin_path = entry_path.join(".clawhub").join("origin.json");
    let content = std::fs::read_to_string(origin_path).ok()?;
    serde_json::from_str::<SkillOriginRecord>(&content).ok()
}

fn collect_managed_skills() -> Vec<SkillInfo> {
    let skills_dir = format!("{}/skills", get_openclaw_home());
    let path = Path::new(&skills_dir);
    if !path.exists() || !path.is_dir() {
        return vec![];
    }

    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if !entry_path.is_dir() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            let mut version = String::from("unknown");
            let mut description = String::new();
            let mut enabled = true;

            let pkg_json = entry_path.join("package.json");
            if pkg_json.exists() {
                if let Ok(content) = std::fs::read_to_string(&pkg_json) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(value) = json.get("version").and_then(|value| value.as_str()) {
                            version = value.to_string();
                        }
                        if let Some(value) =
                            json.get("description").and_then(|value| value.as_str())
                        {
                            description = value.to_string();
                        }
                    }
                }
            }

            for manifest_name in &["manifest.yaml", "manifest.yml", "skill.yaml", "skill.yml"] {
                let manifest = entry_path.join(manifest_name);
                if manifest.exists() {
                    if let Ok(content) = std::fs::read_to_string(&manifest) {
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed.starts_with("version:") {
                                version = trimmed
                                    .trim_start_matches("version:")
                                    .trim()
                                    .trim_matches('"')
                                    .trim_matches('\'')
                                    .to_string();
                            } else if trimmed.starts_with("description:") {
                                description = trimmed
                                    .trim_start_matches("description:")
                                    .trim()
                                    .trim_matches('"')
                                    .trim_matches('\'')
                                    .to_string();
                            } else if trimmed.starts_with("enabled:") {
                                enabled = trimmed.trim_start_matches("enabled:").trim() != "false";
                            }
                        }
                    }
                    break;
                }
            }

            let origin = read_skill_origin(&entry_path);
            skills.push(SkillInfo {
                name,
                version,
                description,
                path: entry_path.to_string_lossy().to_string(),
                enabled,
                origin_registry: origin.as_ref().and_then(|value| value.registry.clone()),
                origin_slug: origin.as_ref().and_then(|value| value.slug.clone()),
                installed_version: origin.and_then(|value| value.installed_version),
            });
        }
    }

    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

fn resolve_skill_marketplace_preset(
    source: Option<&str>,
) -> Result<SkillMarketplacePreset, String> {
    match source
        .unwrap_or("tencent")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "tencent" | "skillhub" | "skillhub-tencent" => Ok(SkillMarketplacePreset {
            id: "tencent",
            label: "腾讯 SkillHub",
        }),
        other => Err(format!("当前版本仅支持腾讯 SkillHub，收到来源: {}", other)),
    }
}

fn clamp_skill_marketplace_limit(limit: Option<u32>, fallback: u32) -> u32 {
    limit.unwrap_or(fallback).clamp(1, 24)
}

fn parse_timestamp_value(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
}

fn tencent_skillhub_summary(record: &TencentSkillHubSkillRecord) -> String {
    record
        .description_zh
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            record
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default()
        .to_string()
}

fn parse_tencent_skillhub_entries(
    payload: &str,
    preset: SkillMarketplacePreset,
    limit: usize,
) -> Result<Vec<SkillMarketplaceEntry>, String> {
    let parsed = serde_json::from_str::<TencentSkillHubResponse>(payload)
        .map_err(|error| format!("解析 {} 数据失败: {}", preset.label, error))?;

    if parsed.code != 0 {
        let message = parsed
            .message
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("未知错误");
        return Err(format!("{} 返回异常: {}", preset.label, message));
    }

    let mut entries = parsed
        .data
        .skills
        .into_iter()
        .filter_map(|record| {
            let slug = record
                .slug
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_string())?;
            let display_name = record
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&slug)
                .to_string();

            Some(SkillMarketplaceEntry {
                slug,
                display_name,
                summary: tencent_skillhub_summary(&record),
                version: record
                    .version
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string()),
                updated_at: record.updated_at.as_ref().and_then(parse_timestamp_value),
                marketplace: preset.id.to_string(),
                marketplace_label: preset.label.to_string(),
            })
        })
        .collect::<Vec<_>>();

    entries.truncate(limit);
    Ok(entries)
}

fn write_skill_origin_record(
    entry_dir: &Path,
    registry: &str,
    slug: &str,
    installed_version: Option<&str>,
) -> Result<(), String> {
    let origin_dir = entry_dir.join(".clawhub");
    std::fs::create_dir_all(&origin_dir)
        .map_err(|error| format!("写入 Skill 来源信息失败: {}", error))?;

    let record = SkillOriginRecord {
        registry: Some(registry.to_string()),
        slug: Some(slug.to_string()),
        installed_version: installed_version
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string()),
    };
    let payload = serde_json::to_string_pretty(&record)
        .map_err(|error| format!("序列化 Skill 来源信息失败: {}", error))?;
    std::fs::write(origin_dir.join("origin.json"), format!("{}\n", payload))
        .map_err(|error| format!("写入 Skill 来源信息失败: {}", error))
}

fn ensure_tencent_skillhub_cli() -> Result<(), String> {
    refresh_path();
    if command_exists("skillhub") {
        return Ok(());
    }

    let install_script = format!(
        "curl -fsSL {} | bash -s -- --cli-only --no-skills",
        TENCENT_SKILLHUB_INSTALL_SCRIPT_URL
    );
    let install_result = run_cmd_owned_timeout(
        "bash",
        &["-lc".to_string(), install_script],
        Duration::from_secs(240),
    );
    if !install_result.success {
        let detail = if install_result.stderr.trim().is_empty() {
            install_result.stdout.trim().to_string()
        } else {
            install_result.stderr.trim().to_string()
        };
        return Err(if detail.is_empty() {
            "安装腾讯 SkillHub CLI 失败".to_string()
        } else {
            format!("安装腾讯 SkillHub CLI 失败: {}", detail)
        });
    }

    refresh_path();
    if command_exists("skillhub") {
        Ok(())
    } else {
        Err("腾讯 SkillHub CLI 安装完成，但当前环境仍未找到 `skillhub` 命令".to_string())
    }
}

fn build_skills_dashboard_snapshot() -> SkillsDashboardSnapshot {
    let managed_skills = collect_managed_skills();
    let managed_dir = format!("{}/skills", get_openclaw_home());
    let workspace_dir = format!("{}/workspace", get_openclaw_home());
    let config = read_openclaw_config().unwrap_or_else(|| serde_json::json!({}));
    let mut warnings = Vec::new();
    let managed_lookup = build_managed_skill_lookup(&managed_skills);

    let mut openclaw_skills = Vec::new();
    if let Some(bundled_dir) = resolve_openclaw_skills_dir("openclaw-bundled") {
        openclaw_skills.extend(collect_openclaw_skills_from_dir(
            &bundled_dir,
            "openclaw-bundled",
            &managed_lookup,
            &config,
            &mut warnings,
        ));
    } else {
        warnings.push("未找到 OpenClaw 自带 Skills 目录".to_string());
    }
    if let Some(workspace_skills_dir) = resolve_openclaw_skills_dir("openclaw-workspace") {
        openclaw_skills.extend(collect_openclaw_skills_from_dir(
            &workspace_skills_dir,
            "openclaw-workspace",
            &managed_lookup,
            &config,
            &mut warnings,
        ));
    }

    openclaw_skills.sort_by(|left, right| {
        right
            .eligible
            .cmp(&left.eligible)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.name.cmp(&right.name))
    });

    SkillsDashboardSnapshot {
        workspace_dir,
        managed_skills_dir: managed_dir,
        managed_skills,
        summary: SkillsDashboardSummary {
            managed_count: managed_lookup
                .values()
                .map(|skill| skill.path.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            bundled_count: openclaw_skills
                .iter()
                .filter(|skill| skill.source == "openclaw-bundled")
                .count(),
            workspace_count: openclaw_skills
                .iter()
                .filter(|skill| skill.source == "openclaw-workspace")
                .count(),
            eligible_count: openclaw_skills.iter().filter(|skill| skill.eligible).count(),
            missing_requirement_count: openclaw_skills
                .iter()
                .filter(|skill| !skill.missing.is_empty())
                .count(),
        },
        openclaw_skills,
        warnings,
    }
}

fn build_skills_requirement_snapshot() -> SkillsRequirementSnapshot {
    let mut warnings = Vec::new();
    let check_args = vec![
        "skills".to_string(),
        "check".to_string(),
        "--json".to_string(),
    ];
    let check_payload = parse_command_json::<OpenClawSkillsCheckResponse>(
        run_openclaw_args_timeout(&check_args, Duration::from_secs(20)),
        "检查 OpenClaw skills 依赖失败",
    )
    .map_err(|error| warnings.push(error))
    .ok();

    let install_hints_by_skill = check_payload
        .as_ref()
        .map(|payload| {
            payload
                .missing_requirements
                .iter()
                .map(|entry| (entry.name.clone(), entry.install.clone()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    SkillsRequirementSnapshot {
        eligible_count: check_payload
            .as_ref()
            .map(|payload| payload.summary.eligible)
            .unwrap_or(0),
        missing_requirement_count: check_payload
            .as_ref()
            .map(|payload| payload.summary.missing_requirements)
            .unwrap_or(0),
        install_hints_by_skill,
        warnings,
    }
}

#[tauri::command]
pub(crate) fn list_skills() -> Vec<SkillInfo> {
    collect_managed_skills()
}

#[tauri::command]
pub(crate) async fn get_skills_dashboard_snapshot() -> Result<SkillsDashboardSnapshot, String> {
    tokio::task::spawn_blocking(build_skills_dashboard_snapshot)
        .await
        .map_err(|error| format!("Task panic: {}", error))
}

#[tauri::command]
pub(crate) async fn get_skills_requirement_snapshot() -> Result<SkillsRequirementSnapshot, String> {
    tokio::task::spawn_blocking(build_skills_requirement_snapshot)
        .await
        .map_err(|error| format!("Task panic: {}", error))
}

#[tauri::command]
pub(crate) async fn search_skill_marketplace(
    source: Option<String>,
    query: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<SkillMarketplaceEntry>, String> {
    let preset = resolve_skill_marketplace_preset(source.as_deref())?;
    let bounded_limit = clamp_skill_marketplace_limit(limit, 12);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| format!("创建 Skills 市场请求失败: {}", error))?;

    let trimmed_query = query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let url = if let Some(keyword) = trimmed_query.as_deref() {
        let mut url = reqwest::Url::parse(TENCENT_SKILLHUB_API_LIST_URL)
            .map_err(|error| format!("构建 Skills 搜索地址失败: {}", error))?;
        url.query_pairs_mut()
            .append_pair("page", "1")
            .append_pair("pageSize", &bounded_limit.to_string())
            .append_pair("sortBy", "score")
            .append_pair("order", "desc")
            .append_pair("keyword", keyword);
        url
    } else {
        reqwest::Url::parse(TENCENT_SKILLHUB_API_TOP_URL)
            .map_err(|error| format!("构建 Skills 市场地址失败: {}", error))?
    };

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("访问 {} 失败: {}", preset.label, error))?;
    let status = response.status();
    let payload = response
        .text()
        .await
        .map_err(|error| format!("读取 {} 响应失败: {}", preset.label, error))?;

    if !status.is_success() {
        let detail = payload.trim();
        return Err(if detail.is_empty() {
            format!("{} 返回 HTTP {}", preset.label, status.as_u16())
        } else {
            format!("{} 返回 HTTP {}: {}", preset.label, status.as_u16(), detail)
        });
    }

    parse_tencent_skillhub_entries(&payload, preset, bounded_limit as usize)
}

#[tauri::command]
pub(crate) async fn install_skill_from_marketplace(
    source: Option<String>,
    slug: String,
    version: Option<String>,
    force: Option<bool>,
) -> CommandResult {
    if let Err(error) = resolve_skill_marketplace_preset(source.as_deref()) {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: error,
            code: Some(1),
        };
    }

    tokio::task::spawn_blocking(move || {
        let trimmed_slug = slug.trim().to_string();
        if trimmed_slug.is_empty()
            || trimmed_slug.contains('/')
            || trimmed_slug.contains('\\')
            || trimmed_slug.contains("..")
        {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: "请输入合法的 skill slug".into(),
                code: Some(1),
            };
        }

        let requested_version = version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        let skills_dir = PathBuf::from(get_openclaw_home()).join("skills");
        if let Err(error) = std::fs::create_dir_all(&skills_dir) {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("创建 Skills 目录失败: {}", error),
                code: Some(1),
            };
        }

        if let Err(error) = ensure_tencent_skillhub_cli() {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: error,
                code: Some(1),
            };
        }

        let mut args = vec![
            "--dir".to_string(),
            skills_dir.to_string_lossy().to_string(),
            "--index".to_string(),
            TENCENT_SKILLHUB_INDEX_URL.to_string(),
            "--skip-self-upgrade".to_string(),
            "install".to_string(),
            trimmed_slug.clone(),
            "--primary-download-url-template".to_string(),
            TENCENT_SKILLHUB_PRIMARY_DOWNLOAD_URL_TEMPLATE.to_string(),
        ];

        if force.unwrap_or(false) {
            args.push("--force".to_string());
        }

        let result = run_cmd_owned_timeout("skillhub", &args, Duration::from_secs(180));
        if !result.success {
            return result;
        }

        let skill_dir = skills_dir.join(&trimmed_slug);
        match write_skill_origin_record(
            &skill_dir,
            TENCENT_SKILLHUB_SITE_URL,
            &trimmed_slug,
            requested_version.as_deref(),
        ) {
            Ok(()) => result,
            Err(error) => CommandResult {
                success: true,
                stdout: format!("{}\nwarn: {}", result.stdout, error),
                stderr: String::new(),
                code: Some(0),
            },
        }
    })
    .await
    .unwrap_or_else(|error| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", error),
        code: None,
    })
}

#[tauri::command]
pub(crate) async fn install_skill_requirement(
    skill_name: String,
    source: String,
    hint_id: String,
) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let normalized_skill_name = skill_name.trim().to_string();
        let normalized_source = source.trim().to_string();
        let normalized_hint_id = hint_id.trim().to_string();

        if normalized_skill_name.is_empty()
            || normalized_skill_name.contains('/')
            || normalized_skill_name.contains('\\')
            || normalized_skill_name.contains("..")
        {
            return invalid_skill_install_input("请输入合法的 Skill 名称");
        }

        if normalized_hint_id.is_empty() {
            return invalid_skill_install_input("缺少依赖安装提示 ID");
        }

        let recipe = match load_skill_install_recipe(
            &normalized_skill_name,
            &normalized_source,
            &normalized_hint_id,
        ) {
            Ok(recipe) => recipe,
            Err(error) => return invalid_skill_install_input(&error),
        };

        execute_skill_install_recipe(&normalized_skill_name, &recipe)
    })
    .await
    .unwrap_or_else(|error| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", error),
        code: None,
    })
}

#[tauri::command]
pub(crate) fn delete_skill(name: String) -> CommandResult {
    let skill_path = format!("{}/skills/{}", get_openclaw_home(), name);
    let path = std::path::Path::new(&skill_path);
    if !path.exists() {
        return CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("Skill '{}' not found", name),
            code: Some(1),
        };
    }
    match std::fs::remove_dir_all(path) {
        Ok(_) => CommandResult {
            success: true,
            stdout: format!("已删除 {}", name),
            stderr: String::new(),
            code: Some(0),
        },
        Err(e) => CommandResult {
            success: false,
            stdout: String::new(),
            stderr: format!("删除失败: {}", e),
            code: Some(1),
        },
    }
}

#[tauri::command]
pub(crate) async fn install_default_skills(app: AppHandle) -> CommandResult {
    tokio::task::spawn_blocking(move || {
        let _ = app.emit(
            "skill-install-log",
            crate::types::InstallEvent {
                level: "info".into(),
                message: "OpenClaw 当前版本已内置基础 bundled skills。".into(),
            },
        );
        let _ = app.emit(
            "skill-install-log",
            crate::types::InstallEvent {
                level: "info".into(),
                message: "如需更多扩展，请在控制面板的 Skills 页面里按需安装第三方技能。".into(),
            },
        );

        let _ = app.emit(
            "skill-install-log",
            crate::types::InstallEvent {
                level: "done".into(),
                message: "success".into(),
            },
        );

        CommandResult {
            success: true,
            stdout: "基础 bundled skills 已可用；无需再安装旧版默认技能。".into(),
            stderr: String::new(),
            code: Some(0),
        }
    })
    .await
    .unwrap_or_else(|e| CommandResult {
        success: false,
        stdout: String::new(),
        stderr: format!("Task panic: {}", e),
        code: None,
    })
}
