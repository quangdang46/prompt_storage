//! Core data model: prompts, variables, aliases, collections.

use serde::{Deserialize, Serialize};

/// Variable types supported by prompt templates (plan §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VariableType {
    #[default]
    Text,
    Multiline,
    Select,
    File,
    Path,
}

impl VariableType {
    pub fn as_str(self) -> &'static str {
        match self {
            VariableType::Text => "text",
            VariableType::Multiline => "multiline",
            VariableType::Select => "select",
            VariableType::File => "file",
            VariableType::Path => "path",
        }
    }

    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "text" => Some(VariableType::Text),
            "multiline" => Some(VariableType::Multiline),
            "select" => Some(VariableType::Select),
            "file" => Some(VariableType::File),
            "path" => Some(VariableType::Path),
            _ => None,
        }
    }
}

/// A declared variable inside a prompt template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptVariable {
    /// UPPER_SNAKE_CASE name matching the `{{NAME}}` placeholder.
    pub name: String,
    #[serde(rename = "type", default)]
    pub var_type: VariableType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

/// The core prompt record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    /// Canonical kebab-case id: `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
    pub id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// beginner | intermediate | advanced | None
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default)]
    pub featured: bool,
    /// manual | imported
    #[serde(default = "default_source")]
    pub source: String,
    /// Resolution tie-break signal (bumped on prefix/fuzzy hits).
    #[serde(default)]
    pub use_count: i64,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn default_source() -> String {
    "manual".to_string()
}

impl Prompt {
    /// Create a new prompt with required fields; everything else defaults.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            content: content.into(),
            description: None,
            category: None,
            tags: Vec::new(),
            variables: Vec::new(),
            version: None,
            author: None,
            difficulty: None,
            featured: false,
            source: default_source(),
            use_count: 0,
            last_used_at: None,
            created_at: None,
            updated_at: None,
        }
    }
}

/// Summary view of a prompt for list output.
#[derive(Debug, Clone, Serialize)]
pub struct PromptSummary {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub use_count: i64,
}

impl From<&Prompt> for PromptSummary {
    fn from(p: &Prompt) -> Self {
        Self {
            id: p.id.clone(),
            title: p.title.clone(),
            description: p.description.clone(),
            category: p.category.clone(),
            tags: p.tags.clone(),
            use_count: p.use_count,
        }
    }
}

/// A named collection of prompts.
#[derive(Debug, Clone, Serialize)]
pub struct Collection {
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<String>,
}
