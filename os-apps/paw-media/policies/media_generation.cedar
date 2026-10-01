// Authenticated agents can create, inspect, and start MediaGenerationRequest flows.
permit(
  principal,
  action in [
    Action::"create",
    Action::"read",
    Action::"list",
    Action::"Generate",
    Action::"GenerateWithOpenRouter"
  ],
  resource is MediaGenerationRequest
);

// Provider auth can only report auth readiness or auth failure from its WASM module.
permit(
  principal is Agent,
  action in [
    Action::"RecordAuthReady",
    Action::"RecordError"
  ],
  resource is MediaGenerationRequest
) when {
  context.module == "provider_auth_gate"
};

// The Codex renderer is the only module that can store or finalize image results.
permit(
  principal is Agent,
  action in [
    Action::"RecordStoring",
    Action::"RecordResult",
    Action::"RecordError"
  ],
  resource is MediaGenerationRequest
) when {
  context.module == "openai_codex_image_generate"
};

// Allow the provider WASM to call the Codex backend and local OData/PawFS APIs.
permit(
  principal is Agent,
  action == Action::"http_call",
  resource is HttpEndpoint
) when {
  context.module == "openai_codex_image_generate"
};

// Allow the provider WASM to read Codex OAuth/token and Temper API secrets.
permit(
  principal is Agent,
  action == Action::"access_secret",
  resource is Secret
) when {
  context.module == "openai_codex_image_generate"
};

// The OpenRouter renderer stores and finalizes its own results, like the Codex one.
permit(
  principal is Agent,
  action in [
    Action::"RecordStoring",
    Action::"RecordResult",
    Action::"RecordError"
  ],
  resource is MediaGenerationRequest
) when {
  context.module == "openrouter_image_generate"
};

// Allow the OpenRouter renderer to call OpenRouter and local OData/PawFS APIs.
permit(
  principal is Agent,
  action == Action::"http_call",
  resource is HttpEndpoint
) when {
  context.module == "openrouter_image_generate"
};

// Allow the OpenRouter renderer to read its API key and the Temper API secrets.
permit(
  principal is Agent,
  action == Action::"access_secret",
  resource is Secret
) when {
  context.module == "openrouter_image_generate"
};
