interface AgentSetupStatus {
  has_anthropic_key: boolean;
  has_agents: boolean;
  has_personalized_soul: boolean;
}

export function requiresAgentSetup(path: string, setup: AgentSetupStatus): boolean {
  // Auth is checked before this setup guard. Research and Foresight records
  // remain inspectable before unrelated first-run setup is complete.
  const isSessionDetail = /^\/sessions\/[A-Za-z0-9_-]+\/?$/.test(path);
  const isForesightDetail = /^\/entities\/(Worlds|EventNodes|Endpoints|Claims|Paths|Forecasts|LearningRuns)\/[A-Za-z0-9_-]+\/?$/.test(path);
  return !isSessionDetail && !isForesightDetail && path !== '/welcome' && path !== '/foresight' && path !== '/settings'
    && (!setup.has_anthropic_key || !setup.has_agents || !setup.has_personalized_soul);
}
