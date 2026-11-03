// OpenAICodexAuth — Cedar Authorization Policies
//
// The OpenAI Codex sign-in. TemperPaw's /paw/setup/openai-codex/* routes make
// each move as their caller (ADR-038): a person or the dashboard through
// Admin, and provider_auth_gate, which calls those routes as agent_type
// "system" when a Session needs a fresh login. The callbacks the
// openai_codex_auth module answers with run as that same caller. Without
// these permits every one of those moves was refused ("no matching permit
// policy"), so a Session could not ask for a sign-in or poll one.
//
// The tokens are not in this record: the module saves them as tenant secrets.

permit(
  principal is Admin,
  action,
  resource is OpenAICodexAuth
);

permit(
  principal,
  action,
  resource is OpenAICodexAuth
) when {
  principal has agent_type &&
  ["supervisor", "human", "system"].contains(principal.agent_type)
};
