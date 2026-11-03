//! LLM calls during setup — multi-provider support for soul generation.
//!
//! Detects provider from API key prefix and uses the appropriate API format.
//! Supports Anthropic, OpenRouter, OpenAI, and OpenAI-compatible endpoints.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Detected LLM provider with its configuration.
pub enum LlmProvider {
    Anthropic {
        api_key: String,
        model: String,
    },
    OpenRouter {
        api_key: String,
        model: String,
    },
    OpenAi {
        api_key: String,
        model: String,
    },
    OpenAiCompatible {
        provider: String,
        api_key: String,
        api_url: String,
        model: String,
    },
}

impl LlmProvider {
    /// Detect provider from API key prefix.
    pub fn detect(api_key: &str, provider_hint: &str, model: &str) -> Result<Self> {
        if model.trim().is_empty() {
            anyhow::bail!("LLM model is required for setup LLM calls");
        }
        let provider_hint = normalize_provider_hint(provider_hint);
        if api_key.starts_with("sk-ant-") || provider_hint == "anthropic" {
            Ok(LlmProvider::Anthropic {
                api_key: api_key.to_string(),
                model: model.to_string(),
            })
        } else if api_key.starts_with("sk-or-") || provider_hint == "openrouter" {
            Ok(LlmProvider::OpenRouter {
                api_key: api_key.to_string(),
                model: model.to_string(),
            })
        } else if let Some(api_url) = default_openai_compatible_url(&provider_hint) {
            Ok(LlmProvider::OpenAiCompatible {
                provider: provider_hint,
                api_key: api_key.to_string(),
                api_url: api_url.to_string(),
                model: model.to_string(),
            })
        } else {
            Ok(LlmProvider::OpenAi {
                api_key: api_key.to_string(),
                model: model.to_string(),
            })
        }
    }

    pub fn openai_compatible(
        provider: &str,
        api_key: &str,
        api_url: &str,
        model: &str,
    ) -> Result<Self> {
        if model.trim().is_empty() {
            anyhow::bail!("LLM model is required for setup LLM calls");
        }
        if api_url.trim().is_empty() {
            anyhow::bail!("{provider} requires an OpenAI-compatible API URL");
        }
        Ok(Self::OpenAiCompatible {
            provider: normalize_provider_hint(provider),
            api_key: api_key.to_string(),
            api_url: api_url.to_string(),
            model: model.to_string(),
        })
    }

    /// Make an LLM call and return the text response.
    pub async fn call(&self, system: &str, user_msg: &str, max_tokens: u32) -> Result<String> {
        let client = reqwest::Client::new();

        match self {
            LlmProvider::Anthropic { api_key, model } => {
                let body = serde_json::json!({
                    "model": model,
                    "max_tokens": max_tokens,
                    "system": system,
                    "messages": [{"role": "user", "content": user_msg}]
                });
                let resp = client
                    .post("https://api.anthropic.com/v1/messages")
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .context("Anthropic API call failed")?;

                let status = resp.status();
                let data: serde_json::Value = resp
                    .json()
                    .await
                    .context("Failed to parse Anthropic response")?;
                if !status.is_success() {
                    let msg = data["error"]["message"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("Anthropic API error ({}): {}", status, msg);
                }
                data["content"][0]["text"]
                    .as_str()
                    .map(|s| s.to_string())
                    .context("No text in Anthropic response")
            }
            LlmProvider::OpenRouter { api_key, model } => {
                let body = serde_json::json!({
                    "model": model,
                    "max_tokens": max_tokens,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user_msg}
                    ]
                });
                let resp = client
                    .post("https://openrouter.ai/api/v1/chat/completions")
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .context("OpenRouter API call failed")?;

                let status = resp.status();
                let data: serde_json::Value = resp
                    .json()
                    .await
                    .context("Failed to parse OpenRouter response")?;
                if !status.is_success() {
                    let msg = data["error"]["message"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("OpenRouter API error ({}): {}", status, msg);
                }
                data["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.to_string())
                    .context("No content in OpenRouter response")
            }
            LlmProvider::OpenAi { api_key, model } => {
                let body = serde_json::json!({
                    "model": model,
                    "max_tokens": max_tokens,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user_msg}
                    ]
                });
                let resp = client
                    .post("https://api.openai.com/v1/chat/completions")
                    .header("Authorization", format!("Bearer {api_key}"))
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .context("OpenAI API call failed")?;

                let status = resp.status();
                let data: serde_json::Value = resp
                    .json()
                    .await
                    .context("Failed to parse OpenAI response")?;
                if !status.is_success() {
                    let msg = data["error"]["message"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("OpenAI API error ({}): {}", status, msg);
                }
                data["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.to_string())
                    .context("No content in OpenAI response")
            }
            LlmProvider::OpenAiCompatible {
                provider,
                api_key,
                api_url,
                model,
            } => {
                let body = serde_json::json!({
                    "model": model,
                    "max_tokens": max_tokens,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user_msg}
                    ]
                });
                let mut request = client
                    .post(api_url)
                    .header("content-type", "application/json")
                    .json(&body);
                if !api_key.trim().is_empty() {
                    request = request.header("Authorization", format!("Bearer {api_key}"));
                }
                let resp = request
                    .send()
                    .await
                    .with_context(|| format!("{provider} API call failed"))?;

                let status = resp.status();
                let data: serde_json::Value = resp
                    .json()
                    .await
                    .with_context(|| format!("Failed to parse {provider} response"))?;
                if !status.is_success() {
                    let msg = data["error"]["message"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("{provider} API error ({}): {}", status, msg);
                }
                data["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.to_string())
                    .with_context(|| format!("No content in {provider} response"))
            }
        }
    }
}

fn normalize_provider_hint(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "hf" | "hugging_face" | "hugging-face" => "huggingface".to_string(),
        "fireworks_ai" | "fireworks-ai" => "fireworks".to_string(),
        "sakana" | "sakana-fugu" | "fugu" => "sakana_fugu".to_string(),
        "ollama" | "local" | "local-openai" => "local_openai".to_string(),
        "openai-compatible" | "openai_compat" | "openai-compat" | "custom_openai" => {
            "openai_compatible".to_string()
        }
        other => other.to_string(),
    }
}

fn default_openai_compatible_url(provider: &str) -> Option<&'static str> {
    match provider {
        "huggingface" => Some("https://router.huggingface.co/v1/chat/completions"),
        "fireworks" => Some("https://api.fireworks.ai/inference/v1/chat/completions"),
        "local_openai" => Some("http://127.0.0.1:11434/v1/chat/completions"),
        _ => None,
    }
}

/// Interview data collected from the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInterview {
    pub name: String,
    pub about_you: String,
    pub ideal_paw: String,
    pub followup_answers: Vec<(String, String)>,
}

/// Generated soul artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSoul {
    pub soul_md: String,
    pub style_md: String,
    pub user_md: String,
    pub summary: String,
}

const FOLLOWUP_SYSTEM: &str = "\
You're getting to know someone who's setting up an AI teammate. They just told you about themselves and what they want. Ask 1-2 follow-up questions that:

- Are specific to what they said, not generic
- Reveal something they didn't think to mention — a value, a tension, a preference
- Feel fun and genuine, not like a corporate questionnaire
- Are short (one sentence each)

Output ONLY the questions, one per line, prefixed with Q:";

const SOUL_SYSTEM: &str = "\
You are generating a personalized agent soul for Temper Paw.

Temper Paw's chief of staff agent (\"Paw\") manages software projects and creative work through specialized project leads. Paw doesn't write code directly — it sets direction, crafts the right lead for each project, and ensures things land. But Paw adapts deeply to the human it works with.

The human below has told you about themselves and what kind of Paw they want. Generate content that is deeply personalized — not a template with blanks filled in, but a soul that reads like it was written by someone who knows this person.

Output exactly four sections separated by these markers on their own lines:

---SOUL---
A SOUL.md following this structure:
## Who I Am
## Worldview (2-4 beliefs specific to this human's domain and values)
## Opinions (on engineering, planning, communication, scope — adapted to this human)
## How I Think
## Tensions I Hold
## Boundaries

---STYLE---
A STYLE.md covering:
## Voice (register, density, confidence — matched to what the human wants)
## Sentence Structure
## Vocabulary (domain-specific terms, words to avoid)
## What Right Sounds Like (2-3 example lines in this Paw's voice)
## What Wrong Sounds Like (1-2 anti-examples)

---USER---
A USER.md profile of the human:
## [Their Name]
### Who They Are
### What They Value
### How They Work
### What They Need From Paw

---SUMMARY---
2-3 sentences describing what this Paw will be like, written directly to the human in second person. This is shown as a preview before they commit. Make it vivid and specific, not generic.";

/// Generate 1-2 follow-up questions based on the user's initial answers.
pub async fn generate_followup_questions(
    provider: &LlmProvider,
    name: &str,
    about_you: &str,
    ideal_paw: &str,
) -> Result<Vec<String>> {
    let user_msg = format!(
        "Name: {name}\n\nAbout themselves:\n{about_you}\n\nWhat kind of Paw they want:\n{ideal_paw}"
    );

    let response = provider.call(FOLLOWUP_SYSTEM, &user_msg, 512).await?;

    let questions: Vec<String> = response
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if let Some(q) = trimmed.strip_prefix("Q:") {
                let q = q.trim();
                if !q.is_empty() {
                    return Some(q.to_string());
                }
            }
            None
        })
        .take(2)
        .collect();

    if questions.is_empty() {
        anyhow::bail!("No follow-up questions generated");
    }
    Ok(questions)
}

/// Generate the full personalized soul from all interview data.
pub async fn generate_personalized_soul(
    provider: &LlmProvider,
    interview: &UserInterview,
) -> Result<GeneratedSoul> {
    let mut user_msg = format!(
        "Name: {}\n\nAbout themselves:\n{}\n\nWhat kind of Paw they want:\n{}",
        interview.name, interview.about_you, interview.ideal_paw
    );

    if !interview.followup_answers.is_empty() {
        user_msg.push_str("\n\nFollow-up answers:");
        for (question, answer) in &interview.followup_answers {
            user_msg.push_str(&format!("\n\nQ: {question}\nA: {answer}"));
        }
    }

    let response = provider.call(SOUL_SYSTEM, &user_msg, 4096).await?;
    parse_generated_soul(&response)
}

/// Generate a refined soul based on user feedback on a previous generation.
pub async fn refine_soul(
    provider: &LlmProvider,
    interview: &UserInterview,
    previous_summary: &str,
    feedback: &str,
) -> Result<GeneratedSoul> {
    let mut user_msg = format!(
        "Name: {}\n\nAbout themselves:\n{}\n\nWhat kind of Paw they want:\n{}",
        interview.name, interview.about_you, interview.ideal_paw
    );

    for (question, answer) in &interview.followup_answers {
        user_msg.push_str(&format!("\n\nQ: {question}\nA: {answer}"));
    }

    user_msg.push_str(&format!(
        "\n\n---\nPrevious attempt summary: {previous_summary}\n\nUser feedback: {feedback}\n\nRegenerate the soul incorporating this feedback."
    ));

    let response = provider.call(SOUL_SYSTEM, &user_msg, 4096).await?;
    parse_generated_soul(&response)
}

fn parse_generated_soul(response: &str) -> Result<GeneratedSoul> {
    let extract = |start: &str, end: Option<&str>| -> String {
        let Some(start_idx) = response.find(start) else {
            return String::new();
        };
        let content_start = start_idx + start.len();
        let content_end = end
            .and_then(|e| response[content_start..].find(e).map(|i| content_start + i))
            .unwrap_or(response.len());
        response[content_start..content_end].trim().to_string()
    };

    let soul_md = extract("---SOUL---", Some("---STYLE---"));
    let style_md = extract("---STYLE---", Some("---USER---"));
    let user_md = extract("---USER---", Some("---SUMMARY---"));
    let summary = extract("---SUMMARY---", None);

    if soul_md.is_empty() || summary.is_empty() {
        anyhow::bail!("Failed to parse generated soul — missing sections");
    }

    Ok(GeneratedSoul {
        soul_md,
        style_md,
        user_md,
        summary,
    })
}
