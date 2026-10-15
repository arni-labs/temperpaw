//! Session Tree Library — shared JSONL tree operations for Agent WASM modules.
//!
//! Provides append-only tree-structured conversation storage with branching,
//! compaction support, and leaf-to-root context assembly.
//!
//! Storage format: JSONL (one JSON object per line) with tree structure via
//! id/parentId. Entries can carry content inline or reference TemperFS files
//! through `content_file_id`, with optional immutable `content_file_version_id`
//! refs for stable historical reads.

use serde_json::{Value, json};
use std::collections::BTreeMap;

/// A single entry in the session tree.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub entry_type: EntryType,
    pub data: Value,
    pub tokens: usize,
    pub content_file_id: Option<String>,
    pub content_file_version_id: Option<String>,
}

/// Type of session tree entry.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryType {
    /// Session header with metadata.
    Header,
    /// A conversation message (user, assistant, or tool_result).
    Message,
    /// A compaction summary replacing older messages.
    Compaction,
    /// A steering injection point.
    Steering,
}

impl EntryType {
    pub fn as_str(&self) -> &str {
        match self {
            EntryType::Header => "header",
            EntryType::Message => "message",
            EntryType::Compaction => "compaction",
            EntryType::Steering => "steering",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "header" => EntryType::Header,
            "message" => EntryType::Message,
            "compaction" => EntryType::Compaction,
            "steering" => EntryType::Steering,
            _ => EntryType::Message,
        }
    }
}

/// A reference to a context entry for building LLM messages.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextRef {
    pub entry_id: String,
    pub role: String,
    pub content_file_id: Option<String>,
    pub content_file_version_id: Option<String>,
    pub entry_type: EntryType,
    pub inline_content: Option<Value>,
    pub inline_summary: Option<String>,
}

/// Delta refs between a previously prepared leaf and the current leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextRefDelta {
    pub refs: Vec<ContextRef>,
    pub includes_compaction: bool,
}

/// The session tree — an append-only tree of conversation entries.
pub struct SessionTree {
    entries: BTreeMap<String, SessionEntry>,
    /// Ordered list of entry IDs (insertion order).
    order: Vec<String>,
    /// Raw JSONL lines for serialization.
    raw_lines: Vec<String>,
}

impl SessionTree {
    /// Parse a JSONL string into a SessionTree.
    pub fn from_jsonl(data: &str) -> Self {
        let mut entries = BTreeMap::new();
        let mut order = Vec::new();
        let mut raw_lines = Vec::new();

        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            raw_lines.push(line.to_string());

            if let Ok(val) = serde_json::from_str::<Value>(line) {
                let id = val
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let parent_id = val
                    .get("parentId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let entry_type = val
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(EntryType::from_str)
                    .unwrap_or(EntryType::Message);
                let tokens = val.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let content_file_id = val
                    .get("content_file_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let content_file_version_id = val
                    .get("content_file_version_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                if !id.is_empty() {
                    let entry = SessionEntry {
                        id: id.clone(),
                        parent_id,
                        entry_type,
                        data: val,
                        tokens,
                        content_file_id,
                        content_file_version_id,
                    };
                    order.push(id.clone());
                    entries.insert(id, entry);
                }
            }
        }

        SessionTree {
            entries,
            order,
            raw_lines,
        }
    }

    /// Create an empty session tree with a header entry.
    pub fn new(session_id: &str) -> Self {
        let header = json!({
            "id": format!("h-{session_id}"),
            "parentId": null,
            "type": "header",
            "version": 1,
            "created": "",
            "tokens": 0
        });
        let header_line = serde_json::to_string(&header).unwrap_or_default();
        let id = format!("h-{session_id}");

        let entry = SessionEntry {
            id: id.clone(),
            parent_id: None,
            entry_type: EntryType::Header,
            data: header,
            tokens: 0,
            content_file_id: None,
            content_file_version_id: None,
        };

        let mut entries = BTreeMap::new();
        entries.insert(id.clone(), entry);

        SessionTree {
            entries,
            order: vec![id],
            raw_lines: vec![header_line],
        }
    }

    /// Check if the tree is empty (no entries at all).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Get an entry by ID.
    pub fn get(&self, id: &str) -> Option<&SessionEntry> {
        self.entries.get(id)
    }

    /// Find the last entry ID (the most recently appended).
    pub fn last_entry_id(&self) -> Option<&str> {
        self.order.last().map(|s| s.as_str())
    }

    /// Resolve a requested leaf to a leaf that can actually produce LLM context.
    ///
    /// Session.entity `session_leaf_id` can drift past durable SessionEntry
    /// rows during continuation/recovery races. In that state callers should
    /// fall back to the newest entry whose parent chain is fully present,
    /// rather than feeding an empty conversation to the model.
    pub fn resolve_context_leaf_id(&self, requested_leaf_id: &str) -> Option<String> {
        if !requested_leaf_id.is_empty() && self.context_leaf_is_walkable(requested_leaf_id) {
            return Some(requested_leaf_id.to_string());
        }

        self.latest_context_leaf_id().map(str::to_string)
    }

    /// Find the newest leaf whose parent chain is present and has non-header
    /// context content.
    pub fn latest_context_leaf_id(&self) -> Option<&str> {
        self.order
            .iter()
            .rev()
            .map(String::as_str)
            .find(|entry_id| self.context_leaf_is_walkable(entry_id))
    }

    /// True when walking from this leaf reaches root without a missing parent
    /// and yields at least one non-header context ref.
    pub fn context_leaf_is_walkable(&self, leaf_id: &str) -> bool {
        let Some(chain) = self.chain_from_leaf(leaf_id) else {
            return false;
        };
        !context_refs_from_chain(&chain).refs.is_empty()
    }

    /// Build context messages by walking from leaf_id to root.
    /// This only reads inline content; callers that want file-backed content
    /// should use `build_context_refs`.
    pub fn build_context(&self, leaf_id: &str) -> Vec<Value> {
        let mut chain: Vec<&SessionEntry> = Vec::new();
        let mut current_id = Some(leaf_id.to_string());

        while let Some(id) = current_id {
            if let Some(entry) = self.entries.get(&id) {
                chain.push(entry);
                current_id = entry.parent_id.clone();
            } else {
                break;
            }
        }

        chain.reverse();

        let mut messages: Vec<Value> = Vec::new();

        for entry in &chain {
            match entry.entry_type {
                EntryType::Header => continue,
                EntryType::Compaction => {
                    messages.clear();
                    if let Some(summary) = entry.data.get("summary").and_then(|v| v.as_str()) {
                        messages.push(json!({
                            "role": "user",
                            "content": format!("[Previous conversation summary]\n{summary}")
                        }));
                    }
                }
                EntryType::Message | EntryType::Steering => {
                    let role = entry
                        .data
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("user");
                    if let Some(content) = entry.data.get("content").cloned() {
                        messages.push(json!({
                            "role": role,
                            "content": content,
                        }));
                    }
                }
            }
        }

        messages
    }

    /// Build context entry refs by walking from leaf_id to root.
    pub fn build_context_refs(&self, leaf_id: &str) -> Vec<ContextRef> {
        let Some(chain) = self.chain_from_leaf(leaf_id) else {
            return Vec::new();
        };
        context_refs_from_chain(&chain).refs
    }

    /// Build the refs between `after_entry_id` and `leaf_id` when the former
    /// is an ancestor of the latter. Returns `None` when ancestry diverges.
    pub fn build_context_refs_since(
        &self,
        leaf_id: &str,
        after_entry_id: &str,
    ) -> Option<ContextRefDelta> {
        let chain = self.chain_after_ancestor(leaf_id, after_entry_id)?;
        Some(context_refs_from_chain(&chain))
    }

    /// Append a new entry to the tree. Returns the JSONL line for the new entry.
    pub fn append_entry(
        &mut self,
        id: &str,
        parent_id: Option<&str>,
        entry_type: EntryType,
        role: Option<&str>,
        content: Option<&Value>,
        tokens: usize,
        extra_fields: Option<&Value>,
    ) -> String {
        let mut data = json!({
            "id": id,
            "parentId": parent_id,
            "type": entry_type.as_str(),
            "tokens": tokens,
        });

        if let Some(role) = role {
            data["role"] = json!(role);
        }
        if let Some(content) = content {
            data["content"] = content.clone();
        }
        if let Some(extra) = extra_fields {
            if let Some(obj) = extra.as_object() {
                for (k, v) in obj {
                    data[k] = v.clone();
                }
            }
        }

        let content_file_id = data
            .get("content_file_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let content_file_version_id = data
            .get("content_file_version_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let line = serde_json::to_string(&data).unwrap_or_default();

        let entry = SessionEntry {
            id: id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            entry_type,
            data,
            tokens,
            content_file_id,
            content_file_version_id,
        };

        self.order.push(id.to_string());
        self.entries.insert(id.to_string(), entry);
        self.raw_lines.push(line.clone());

        line
    }

    /// Append a new entry with content stored in a TemperFS file.
    pub fn append_entry_with_file(
        &mut self,
        id: &str,
        parent_id: Option<&str>,
        entry_type: EntryType,
        role: Option<&str>,
        content_file_id: &str,
        content_file_version_id: Option<&str>,
        tokens: usize,
        extra_fields: Option<&Value>,
    ) -> String {
        let extra = if let Some(existing) = extra_fields {
            let mut obj = existing.clone();
            obj["content_file_id"] = json!(content_file_id);
            if let Some(version_id) = content_file_version_id.filter(|value| !value.is_empty()) {
                obj["content_file_version_id"] = json!(version_id);
            }
            Some(obj)
        } else {
            let mut obj = json!({ "content_file_id": content_file_id });
            if let Some(version_id) = content_file_version_id.filter(|value| !value.is_empty()) {
                obj["content_file_version_id"] = json!(version_id);
            }
            Some(obj)
        };
        self.append_entry(
            id,
            parent_id,
            entry_type,
            role,
            None,
            tokens,
            extra.as_ref(),
        )
    }

    /// Append a user message. Returns (entry_id, jsonl_line).
    pub fn append_user_message(
        &mut self,
        parent_id: &str,
        content: &str,
        tokens: usize,
    ) -> (String, String) {
        let id = format!("u-{}", self.order.len());
        let line = self.append_entry(
            &id,
            Some(parent_id),
            EntryType::Message,
            Some("user"),
            Some(&json!(content)),
            tokens,
            None,
        );
        (id, line)
    }

    /// Append a user message with content stored in a TemperFS file.
    pub fn append_user_message_file(
        &mut self,
        parent_id: &str,
        content_file_id: &str,
        content_file_version_id: Option<&str>,
        tokens: usize,
    ) -> (String, String) {
        let id = format!("u-{}", self.order.len());
        let line = self.append_entry_with_file(
            &id,
            Some(parent_id),
            EntryType::Message,
            Some("user"),
            content_file_id,
            content_file_version_id,
            tokens,
            None,
        );
        (id, line)
    }

    /// Append an assistant message. Returns (entry_id, jsonl_line).
    pub fn append_assistant_message(
        &mut self,
        parent_id: &str,
        content: &Value,
        tokens: usize,
    ) -> (String, String) {
        self.append_assistant_message_with_extra(parent_id, content, tokens, None)
    }

    /// Append an assistant message carrying per-turn extras (provider, model,
    /// stop reason, usage, token-level RL signals) the OTS emitter reads later.
    pub fn append_assistant_message_with_extra(
        &mut self,
        parent_id: &str,
        content: &Value,
        tokens: usize,
        extra_fields: Option<&Value>,
    ) -> (String, String) {
        let id = format!("a-{}", self.order.len());
        let line = self.append_entry(
            &id,
            Some(parent_id),
            EntryType::Message,
            Some("assistant"),
            Some(content),
            tokens,
            extra_fields,
        );
        (id, line)
    }

    /// Append an assistant message with content stored in a TemperFS file.
    pub fn append_assistant_message_file(
        &mut self,
        parent_id: &str,
        content_file_id: &str,
        content_file_version_id: Option<&str>,
        tokens: usize,
        extra_fields: Option<&Value>,
    ) -> (String, String) {
        let id = format!("a-{}", self.order.len());
        let line = self.append_entry_with_file(
            &id,
            Some(parent_id),
            EntryType::Message,
            Some("assistant"),
            content_file_id,
            content_file_version_id,
            tokens,
            extra_fields,
        );
        (id, line)
    }

    /// Append a tool result message. Returns (entry_id, jsonl_line).
    pub fn append_tool_results(
        &mut self,
        parent_id: &str,
        tool_results: &Value,
        tokens: usize,
    ) -> (String, String) {
        let id = format!("t-{}", self.order.len());
        let line = self.append_entry(
            &id,
            Some(parent_id),
            EntryType::Message,
            Some("user"),
            Some(tool_results),
            tokens,
            None,
        );
        (id, line)
    }

    /// Append a tool result message with content stored in a TemperFS file.
    pub fn append_tool_results_file(
        &mut self,
        parent_id: &str,
        content_file_id: &str,
        content_file_version_id: Option<&str>,
        tokens: usize,
    ) -> (String, String) {
        let id = format!("t-{}", self.order.len());
        let line = self.append_entry_with_file(
            &id,
            Some(parent_id),
            EntryType::Message,
            Some("user"),
            content_file_id,
            content_file_version_id,
            tokens,
            None,
        );
        (id, line)
    }

    /// Append a compaction entry. Returns (entry_id, jsonl_line).
    pub fn append_compaction(
        &mut self,
        parent_id: &str,
        summary: &str,
        first_kept: &str,
    ) -> (String, String) {
        let id = format!("c-{}", self.order.len());
        let summary_tokens = estimate_summary_tokens(summary);
        let extra = json!({
            "summary": summary,
            "first_kept": first_kept,
        });
        let line = self.append_entry(
            &id,
            Some(parent_id),
            EntryType::Compaction,
            None,
            None,
            summary_tokens,
            Some(&extra),
        );
        (id, line)
    }

    /// Append a compaction entry with summary stored in a TemperFS file.
    pub fn append_compaction_file(
        &mut self,
        parent_id: &str,
        content_file_id: &str,
        content_file_version_id: Option<&str>,
        first_kept: &str,
        summary_tokens: usize,
    ) -> (String, String) {
        let id = format!("c-{}", self.order.len());
        let extra = json!({
            "first_kept": first_kept,
        });
        let line = self.append_entry_with_file(
            &id,
            Some(parent_id),
            EntryType::Compaction,
            None,
            content_file_id,
            content_file_version_id,
            summary_tokens,
            Some(&extra),
        );
        (id, line)
    }

    /// Append a steering message. Returns (entry_id, jsonl_line).
    pub fn append_steering_message(
        &mut self,
        parent_id: &str,
        content: &str,
        tokens: usize,
    ) -> (String, String) {
        let id = format!("s-{}", self.order.len());
        let line = self.append_entry(
            &id,
            Some(parent_id),
            EntryType::Steering,
            Some("user"),
            Some(&json!(content)),
            tokens,
            None,
        );
        (id, line)
    }

    /// Estimate total tokens in the context for a given leaf.
    pub fn estimate_tokens(&self, leaf_id: &str) -> usize {
        let mut total = 0;
        let mut current_id = Some(leaf_id.to_string());

        while let Some(id) = current_id {
            if let Some(entry) = self.entries.get(&id) {
                if entry.entry_type == EntryType::Compaction {
                    total += entry.tokens;
                    break;
                }
                total += entry.tokens;
                current_id = entry.parent_id.clone();
            } else {
                break;
            }
        }

        total
    }

    /// Find a cut point for compaction. Returns the entry ID where we should
    /// start keeping messages (everything before this gets compacted).
    pub fn find_cut_point(&self, leaf_id: &str, keep_recent_tokens: usize) -> Option<String> {
        let mut accumulated = 0;
        let mut current_id = Some(leaf_id.to_string());
        let mut cut_point = None;

        while let Some(id) = current_id {
            if let Some(entry) = self.entries.get(&id) {
                accumulated += entry.tokens;
                if accumulated >= keep_recent_tokens {
                    cut_point = Some(id.clone());
                    break;
                }
                current_id = entry.parent_id.clone();
            } else {
                break;
            }
        }

        cut_point
    }

    /// Serialize the tree back to JSONL format.
    pub fn to_jsonl(&self) -> String {
        self.raw_lines.join("\n")
    }

    /// Get all entry IDs in insertion order.
    pub fn entry_ids(&self) -> &[String] {
        &self.order
    }

    /// If the leaf is an assistant message with tool_use blocks, return synthetic
    /// error tool_results for each tool_use (to handle interrupted execution).
    ///
    /// Returns `None` if the leaf is not an assistant message or has no tool_use blocks.
    pub fn interrupted_tool_results_for_leaf(&self, leaf_id: &str) -> Option<Value> {
        let entry = self.entries.get(leaf_id)?;
        let role = entry.data.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "assistant" {
            return None;
        }

        let blocks = entry.data.get("content").and_then(Value::as_array)?;
        let mut results = Vec::new();
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(tool_use_id) = block.get("id").and_then(Value::as_str) else {
                continue;
            };
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": "Tool execution was interrupted because the previous agent run ended before returning results.",
                "is_error": true,
            }));
        }

        if results.is_empty() {
            None
        } else {
            Some(Value::Array(results))
        }
    }
}

impl SessionTree {
    fn chain_from_leaf(&self, leaf_id: &str) -> Option<Vec<&SessionEntry>> {
        let mut chain: Vec<&SessionEntry> = Vec::new();
        let mut current_id = Some(leaf_id.to_string());

        while let Some(id) = current_id {
            let entry = self.entries.get(&id)?;
            chain.push(entry);
            current_id = entry.parent_id.clone();
        }

        chain.reverse();
        Some(chain)
    }

    fn chain_after_ancestor(
        &self,
        leaf_id: &str,
        after_entry_id: &str,
    ) -> Option<Vec<&SessionEntry>> {
        if !self.entries.contains_key(leaf_id) || !self.entries.contains_key(after_entry_id) {
            return None;
        }

        let mut chain: Vec<&SessionEntry> = Vec::new();
        let mut current_id = Some(leaf_id.to_string());

        while let Some(id) = current_id {
            if id == after_entry_id {
                chain.reverse();
                return Some(chain);
            }

            let entry = self.entries.get(&id)?;
            chain.push(entry);
            current_id = entry.parent_id.clone();
        }

        None
    }
}

fn context_refs_from_chain(chain: &[&SessionEntry]) -> ContextRefDelta {
    let mut refs: Vec<ContextRef> = Vec::new();
    let mut includes_compaction = false;

    for entry in chain {
        match entry.entry_type {
            EntryType::Header => continue,
            EntryType::Compaction => {
                includes_compaction = true;
                refs.clear();
                refs.push(ContextRef {
                    entry_id: entry.id.clone(),
                    role: "user".to_string(),
                    content_file_id: entry.content_file_id.clone(),
                    content_file_version_id: entry.content_file_version_id.clone(),
                    entry_type: EntryType::Compaction,
                    inline_content: None,
                    inline_summary: entry
                        .data
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                });
            }
            EntryType::Message | EntryType::Steering => {
                let role = entry
                    .data
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                refs.push(ContextRef {
                    entry_id: entry.id.clone(),
                    role,
                    content_file_id: entry.content_file_id.clone(),
                    content_file_version_id: entry.content_file_version_id.clone(),
                    entry_type: entry.entry_type.clone(),
                    inline_content: entry.data.get("content").cloned(),
                    inline_summary: None,
                });
            }
        }
    }

    ContextRefDelta {
        refs,
        includes_compaction,
    }
}

fn estimate_summary_tokens(summary: &str) -> usize {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        0
    } else {
        usize::max(1, trimmed.len().div_ceil(4))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_and_build_context_inline() {
        let mut tree = SessionTree::new("test-1");
        let header_id = tree.last_entry_id().unwrap().to_string();
        let (user_id, _) = tree.append_user_message(&header_id, "Hello", 10);
        let (asst_id, _) =
            tree.append_assistant_message(&user_id, &json!([{"type":"text","text":"Hi"}]), 20);

        let messages = tree.build_context(&asst_id);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
    }

    #[test]
    fn headerless_root_user_builds_context_inline() {
        let tree = SessionTree::from_jsonl(
            r#"{"id":"u-ss-1-0","parentId":null,"type":"message","role":"user","content":"hello","tokens":2}
{"id":"a-2","parentId":"u-ss-1-0","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"tokens":3}"#,
        );

        let messages = tree.build_context("a-2");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hello");
        assert_eq!(messages[1]["role"], "assistant");
    }

    #[test]
    fn test_interrupted_tool_results_for_clean_leaf() {
        let mut tree = SessionTree::new("test-r1");
        let header_id = tree.last_entry_id().unwrap().to_string();
        let (user_id, _) = tree.append_user_message(&header_id, "Hello", 10);

        // User leaf — no interrupted tool_use
        assert!(tree.interrupted_tool_results_for_leaf(&user_id).is_none());
    }

    #[test]
    fn test_interrupted_tool_results_for_tool_use_leaf() {
        let mut tree = SessionTree::new("test-r2");
        let header_id = tree.last_entry_id().unwrap().to_string();
        let (user_id, _) = tree.append_user_message(&header_id, "Run some code", 10);
        let (asst_id, _) = tree.append_assistant_message(
            &user_id,
            &json!([
                {"type": "text", "text": "Let me run that."},
                {"type": "tool_use", "id": "tu_1", "name": "bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "tu_2", "name": "read", "input": {"path": "/tmp/x"}},
            ]),
            50,
        );

        let results = tree.interrupted_tool_results_for_leaf(&asst_id);
        assert!(results.is_some());
        let arr = results.unwrap();
        let arr = arr.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["tool_use_id"], "tu_1");
        assert_eq!(arr[0]["is_error"], true);
        assert_eq!(arr[1]["tool_use_id"], "tu_2");
    }

    #[test]
    fn test_interrupted_tool_results_for_text_only_assistant() {
        let mut tree = SessionTree::new("test-r3");
        let header_id = tree.last_entry_id().unwrap().to_string();
        let (user_id, _) = tree.append_user_message(&header_id, "Hi", 5);
        let (asst_id, _) = tree.append_assistant_message(
            &user_id,
            &json!([{"type": "text", "text": "Hello!"}]),
            10,
        );

        // Text-only assistant — no tool_use blocks
        assert!(tree.interrupted_tool_results_for_leaf(&asst_id).is_none());
    }

    #[test]
    fn test_interrupted_tool_results_for_nonexistent_leaf() {
        let tree = SessionTree::new("test-r4");
        assert!(
            tree.interrupted_tool_results_for_leaf("nonexistent")
                .is_none()
        );
    }

    #[test]
    fn test_build_context_refs_for_file_backed_entries() {
        let jsonl = r#"{"id":"h-1","parentId":null,"type":"header","version":1,"tokens":0}
{"id":"u-1","parentId":"h-1","type":"message","role":"user","content_file_id":"file-1","content_file_version_id":"ver-1","tokens":10}
{"id":"a-1","parentId":"u-1","type":"message","role":"assistant","content_file_id":"file-2","content_file_version_id":"ver-2","tokens":5}"#;

        let tree = SessionTree::from_jsonl(jsonl);
        let refs = tree.build_context_refs("a-1");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].content_file_id.as_deref(), Some("file-1"));
        assert_eq!(refs[0].content_file_version_id.as_deref(), Some("ver-1"));
        assert_eq!(refs[1].content_file_id.as_deref(), Some("file-2"));
        assert_eq!(refs[1].content_file_version_id.as_deref(), Some("ver-2"));
    }

    #[test]
    fn test_build_context_refs_since_returns_appendable_delta_for_descendant_leaf() {
        let jsonl = r#"{"id":"h-1","parentId":null,"type":"header","version":1,"tokens":0}
{"id":"u-1","parentId":"h-1","type":"message","role":"user","content":"hello","tokens":10}
{"id":"a-1","parentId":"u-1","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"tokens":5}
{"id":"u-2","parentId":"a-1","type":"message","role":"user","content":"next","tokens":7}"#;

        let tree = SessionTree::from_jsonl(jsonl);
        let delta = tree.build_context_refs_since("u-2", "a-1").unwrap();

        assert!(!delta.includes_compaction);
        assert_eq!(delta.refs.len(), 1);
        assert_eq!(delta.refs[0].entry_id, "u-2");
        assert_eq!(delta.refs[0].role, "user");
    }

    #[test]
    fn test_build_context_refs_since_marks_compaction_in_delta() {
        let jsonl = r#"{"id":"h-1","parentId":null,"type":"header","version":1,"tokens":0}
{"id":"u-1","parentId":"h-1","type":"message","role":"user","content":"hello","tokens":10}
{"id":"a-1","parentId":"u-1","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"tokens":5}
{"id":"c-1","parentId":"a-1","type":"compaction","summary":"summary","first_kept":"a-1","tokens":3}
{"id":"u-2","parentId":"c-1","type":"message","role":"user","content":"next","tokens":7}"#;

        let tree = SessionTree::from_jsonl(jsonl);
        let delta = tree.build_context_refs_since("u-2", "a-1").unwrap();

        assert!(delta.includes_compaction);
        assert_eq!(delta.refs.len(), 2);
        assert_eq!(delta.refs[0].entry_id, "c-1");
        assert_eq!(delta.refs[1].entry_id, "u-2");
    }

    #[test]
    fn test_build_context_refs_since_returns_none_for_divergent_branch() {
        let jsonl = r#"{"id":"h-1","parentId":null,"type":"header","version":1,"tokens":0}
{"id":"u-1","parentId":"h-1","type":"message","role":"user","content":"hello","tokens":10}
{"id":"a-1","parentId":"u-1","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"tokens":5}
{"id":"u-2","parentId":"a-1","type":"message","role":"user","content":"branch-a","tokens":7}
{"id":"u-3","parentId":"a-1","type":"message","role":"user","content":"branch-b","tokens":7}"#;

        let tree = SessionTree::from_jsonl(jsonl);
        assert!(tree.build_context_refs_since("u-2", "u-3").is_none());
    }

    #[test]
    fn resolve_context_leaf_id_falls_back_to_latest_walkable_leaf() {
        let jsonl = r#"{"id":"h-1","parentId":null,"type":"header","version":1,"tokens":0}
{"id":"u-1","parentId":"h-1","type":"message","role":"user","content":"hello","tokens":10}
{"id":"a-1","parentId":"u-1","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"tokens":5}
{"id":"t-1001","parentId":"missing-parent","type":"message","role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"lost"}],"tokens":7}
{"id":"u-1002","parentId":"t-1001","type":"message","role":"user","content":"try again","tokens":3}"#;

        let tree = SessionTree::from_jsonl(jsonl);

        assert!(!tree.context_leaf_is_walkable("t-1001"));
        assert!(!tree.context_leaf_is_walkable("u-1002"));
        assert_eq!(tree.latest_context_leaf_id(), Some("a-1"));
        assert_eq!(
            tree.resolve_context_leaf_id("u-1002").as_deref(),
            Some("a-1")
        );
        assert_eq!(
            tree.resolve_context_leaf_id("missing-leaf").as_deref(),
            Some("a-1")
        );
        assert_eq!(tree.resolve_context_leaf_id("a-1").as_deref(), Some("a-1"));
    }

    #[test]
    fn resolve_context_leaf_id_ignores_header_only_tree() {
        let tree = SessionTree::from_jsonl(
            r#"{"id":"h-1","parentId":null,"type":"header","version":1,"tokens":0}"#,
        );

        assert_eq!(tree.latest_context_leaf_id(), None);
        assert_eq!(tree.resolve_context_leaf_id("h-1"), None);
    }

    #[test]
    fn test_compaction_entries_contribute_to_estimated_tokens() {
        let mut tree = SessionTree::new("test-compaction");
        let header_id = "h-test-compaction".to_string();
        let (user_id, _) = tree.append_user_message(&header_id, "Hello there", 10);
        let (compaction_id, _) =
            tree.append_compaction(&user_id, "Summary of previous work", &user_id);

        assert!(
            tree.estimate_tokens(&compaction_id) > 0,
            "compaction summaries should contribute to future token estimation"
        );
    }
}
