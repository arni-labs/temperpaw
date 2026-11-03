// Webhook ingress is a capability boundary. Only administrators and the two
// named state-machine integrations may inspect or advance accepted envelopes.
permit(
  principal is Admin,
  action in [
    Action::"create",
    Action::"read",
    Action::"list",
    Action::"Received",
    Action::"Routed",
    Action::"Processed",
    Action::"RouteFailed",
    Action::"ProcessFailed"
  ],
  resource is WebhookEvent
);

permit(
  principal is Agent,
  action in [Action::"Routed", Action::"RouteFailed"],
  resource is WebhookEvent
) when {
  context.module == "route_webhook"
};

permit(
  principal is Agent,
  action in [Action::"Processed", Action::"ProcessFailed"],
  resource is WebhookEvent
) when {
  context.module == "process_webhook"
};

// Route records contain target capabilities and vault references. They are
// governed configuration, not user-readable application data.
permit(
  principal is Admin,
  action in [
    Action::"create",
    Action::"read",
    Action::"list",
    Action::"Register",
    Action::"Update",
    Action::"Disable",
    Action::"Enable"
  ],
  resource is WebhookRoute
);

permit(
  principal is Agent,
  action == Action::"http_call",
  resource is HttpEndpoint
) when {
  ["route_webhook", "process_webhook"].contains(context.module)
};
