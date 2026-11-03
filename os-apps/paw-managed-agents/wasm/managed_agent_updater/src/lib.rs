#[path = "../../common.rs"]
mod common;

use common::{
    escape_odata_string, field_i64, field_string, get_entity, managed_tools_enabled, patch_entity,
    post_absolute_action, system_json_headers,
};
use temper_wasm_sdk::prelude::*;
use wasm_helpers::resolve_temper_api_url;

#[unsafe(no_mangle)]
pub extern "C" fn run(_ctx_ptr: i32, _ctx_len: i32) -> i32 {
    let result = (|| -> Result<(), String> {
        let ctx = Context::from_host()?;
        let fields = ctx
            .entity_state
            .get("fields")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let base_url = resolve_temper_api_url(&ctx, &fields);
        let headers = system_json_headers(&ctx, &ctx.tenant, &fields);
        let next_version = next_version(&fields);

        let _ = patch_entity(
            &ctx,
            &base_url,
            &headers,
            "ManagedAgents",
            &ctx.entity_id,
            &json!({ "Version": next_version }),
        )?;

        let inner_agent_id = field_string(&fields, &["InnerAgentId", "inner_agent_id"]);
        if !inner_agent_id.is_empty() {
            let tool_rows = common::list_entities(
                &ctx,
                &base_url,
                &headers,
                &format!(
                    "/tdata/AgentTools?$filter=AgentId%20eq%20'{}'&$orderby=Kind%20asc",
                    escape_odata_string(&ctx.entity_id)
                ),
            )?;
            let tool_config_rows = common::list_entities(
                &ctx,
                &base_url,
                &headers,
                &agent_tool_config_query(&tool_rows),
            )?;
            let tools_enabled = managed_tools_enabled(&tool_rows, &tool_config_rows);
            let current_agent =
                get_entity(&ctx, &base_url, &headers, "ManagedAgents", &ctx.entity_id)?;
            let current_fields = current_agent
                .get("fields")
                .cloned()
                .unwrap_or_else(|| current_agent.clone());
            let model_id = field_string(&current_fields, &["ModelId", "model_id"]);
            if model_id.is_empty() {
                return Err("ManagedAgent requires ModelId before syncing inner Agent".into());
            }
            let provider = field_string(&current_fields, &["Provider", "provider"]);
            if provider.is_empty() {
                return Err("ManagedAgent requires Provider before syncing inner Agent".into());
            }
            let provider_options_json = field_string(
                &current_fields,
                &["ProviderOptionsJson", "provider_options_json"],
            );
            let _ = post_absolute_action(
                &ctx,
                &headers,
                &format!("{base_url}/tdata/Agents('{inner_agent_id}')/TemperPaw.Update"),
                &json!({
                    "description": field_string(&current_fields, &["Description", "description"]),
                    "model": model_id,
                    "provider": provider,
                    "provider_options_json": provider_options_json,
                    "tools_enabled": tools_enabled,
                    "max_turns": "60",
                }),
                "update inner agent after managed-agent change",
            )?;
        }

        temper_wasm_sdk::set_success_result("SyncComplete", &json!({"updated": true}));
        Ok(())
    })();

    if let Err(error) = result {
        temper_wasm_sdk::set_error_result(&error);
    }
    0
}

fn agent_tool_config_query(tool_rows: &[Value]) -> String {
    let tool_ids = tool_rows
        .iter()
        .filter_map(common::entity_id)
        .collect::<Vec<_>>();
    if tool_ids.is_empty() {
        return "/tdata/AgentToolConfigs?$filter=ToolId%20eq%20'__none__'&$orderby=ToolName%20asc"
            .to_string();
    }

    let filter = tool_ids
        .iter()
        .map(|tool_id| format!("ToolId%20eq%20'{}'", escape_odata_string(tool_id)))
        .collect::<Vec<_>>()
        .join("%20or%20");
    format!("/tdata/AgentToolConfigs?$filter={filter}&$orderby=ToolName%20asc")
}

fn next_version(fields: &Value) -> i64 {
    let current = field_i64(fields, &["Version", "version"]);
    if current < 1 { 1 } else { current + 1 }
}
