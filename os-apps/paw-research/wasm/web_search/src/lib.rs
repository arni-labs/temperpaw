//! Web Search — WASM module for searching the web via Exa API.
//!
//! Triggered by WebQuery.ExecuteSearch action. Reads the query from entity state,
//! calls Exa search API, and transitions to Complete with results or Failed with error.
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`

use temper_wasm_sdk::prelude::*;

const TEXT_MAX_CHARACTERS: usize = 5000;

fn search_request(query: &str) -> Value {
    json!({"query": query, "type": "auto", "numResults": 10,
        "contents": {"text": {"maxCharacters": TEXT_MAX_CHARACTERS}}})
}

fn search_result(result: &Value) -> Value {
    let text = result.get("text").and_then(Value::as_str).unwrap_or("");
    json!({
        "title": result.get("title").and_then(Value::as_str).unwrap_or(""),
        "url": result.get("url").and_then(Value::as_str).unwrap_or(""),
        "text": text,
        "text_max_characters": TEXT_MAX_CHARACTERS,
        // Computed from the returned excerpt, not a provider truncation flag.
        // A shorter excerpt does not establish full-source completeness.
        "text_limit_reached": text.chars().count() >= TEXT_MAX_CHARACTERS,
        // Indexed publication metadata is not the observation or retrieval date.
        // Missing metadata remains unknown rather than borrowing today's date.
        "published_at": result.get("publishedDate").and_then(Value::as_str),
    })
}

/// Entry point.
#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        ctx.log("info", "web_search: starting");

        let fields = ctx.entity_state.get("fields").cloned().unwrap_or(json!({}));

        let query = fields.get("query").and_then(|v| v.as_str()).unwrap_or("");

        if query.is_empty() {
            set_success_result(
                "RecordError",
                &json!({"error": "web_search: query is empty"}),
            );
            return Ok(());
        }

        // Resolve Exa API key from integration config
        let exa_api_key = ctx
            .config
            .get("exa_api_key")
            .filter(|s| !s.is_empty() && !s.contains("{secret:"))
            .cloned()
            .unwrap_or_default();

        if exa_api_key.is_empty() {
            set_success_result(
                "RecordError",
                &json!({"error": "web_search: missing exa_api_key secret. Configure EXA_API_KEY."}),
            );
            return Ok(());
        }

        // Build Exa search request
        let body = search_request(query);

        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("x-api-key".to_string(), exa_api_key),
        ];

        ctx.log("info", &format!("web_search: querying Exa for: {query}"));

        let resp = ctx.http_call(
            "POST",
            "https://api.exa.ai/search",
            &headers,
            &body.to_string(),
        )?;

        if resp.status < 200 || resp.status >= 300 {
            let err_body: String = resp.body.chars().take(500).collect();
            set_success_result(
                "RecordError",
                &json!({"error": format!("web_search: Exa API error (HTTP {}): {}", resp.status, err_body)}),
            );
            return Ok(());
        }

        // Parse response
        let parsed: Value = serde_json::from_str(&resp.body)
            .map_err(|e| format!("web_search: failed to parse Exa response: {e}"))?;

        // Extract results into simplified format
        let results = parsed
            .get("results")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().map(search_result).collect::<Vec<_>>())
            .unwrap_or_default();

        let results_json = serde_json::to_string(&results)
            .map_err(|e| format!("web_search: failed to serialize results: {e}"))?;

        ctx.log(
            "info",
            &format!("web_search: got {} results", results.len()),
        );

        set_success_result("RecordResults", &json!({"results": results_json}));

        Ok(())
    })();

    if let Err(e) = result {
        set_error_result(&e);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_unicode_excerpt_limit_are_explicit() {
        let request = search_request("a focused source query");
        assert_eq!(request["contents"]["text"]["maxCharacters"], 5000);
        assert_eq!(request["numResults"], 10);
        assert_eq!(request["query"], "a focused source query");
        for count in [4999, 5000, 5001] {
            let text = "🦀".repeat(count);
            let result = search_result(&json!({"text": text}));
            assert_eq!(result["text"], text);
            assert_eq!(result["text_max_characters"], 5000);
            assert_eq!(result["text_limit_reached"], count >= 5000);
        }
        assert_eq!(search_result(&json!({}))["text_limit_reached"], false);
    }

    #[test]
    fn indexed_publication_date_survives_without_inventing_missing_dates() {
        let historical = search_result(&json!({
            "title": "An older study", "url": "https://example.org/study",
            "text": "Study participants were observed in 2020–2021.",
            "publishedDate": "2025-04-10T00:00:00.000Z"
        }));
        assert_eq!(historical["published_at"], "2025-04-10T00:00:00.000Z");
        assert_eq!(
            historical["text"],
            "Study participants were observed in 2020–2021."
        );
        for missing in [
            json!({}),
            json!({"publishedDate": null}),
            json!({"publishedDate": 2025}),
        ] {
            assert!(search_result(&missing)["published_at"].is_null());
        }
    }
}
