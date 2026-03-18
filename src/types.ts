export interface SystemInfo {
  os: string;
  arch: string;
  node_version: string | null;
  npm_version: string | null;
  pnpm_version: string | null;
  git_version: string | null;
  openclaw_version: string | null;
  total_memory_gb: number;
  free_disk_gb: number;
  openclaw_home_exists: boolean;
  openclaw_home_path: string;
  openclaw_config_exists: boolean;
  openclaw_config_path: string | null;
  openclaw_cli_ok: boolean;
  openclaw_doctor_ok: boolean;
  /** All conditions met: CLI + home dir + config + doctor */
  openclaw_fully_installed: boolean;
  gateway_port: number | null;
  node_ok: boolean;
  memory_ok: boolean;
  memory_recommended: boolean;
  disk_ok: boolean;
}

export interface CommandResult {
  success: boolean;
  stdout: string;
  stderr: string;
  code: number | null;
}

export type AppMode = "loading" | "dashboard" | "wizard";
export type WizardStep = "welcome" | "check" | "configure" | "install";
export type InstallMethod = "npm_mirror" | "official_script";

export interface InstallConfig {
  apiProvider: string;
  apiKey: string;
  apiBaseUrl: string;
  customModelId: string;
  gatewayPort: number;
  installMethod: InstallMethod;
  apiKeyValidated?: boolean;
}

export interface GatewayStatus {
  running: boolean;
  port: number;
  uptime?: string;
  lastCheck?: Date;
  recovering?: boolean;
}
export type DashboardTab = "dashboard" | "channels" | "skills" | "agents" | "models" | "settings";

export interface SkillInfo {
  name: string;
  version: string;
  description: string;
  path: string;
  enabled: boolean;
  originRegistry?: string | null;
  originSlug?: string | null;
  installedVersion?: string | null;
}

export interface SkillRequirementState {
  bins: string[];
  anyBins: string[];
  env: string[];
  config: string[];
  os: string[];
}

export interface SkillInstallHint {
  id: string;
  kind: string;
  label: string;
  bins: string[];
}

export interface OpenClawSkillInfo {
  name: string;
  description: string;
  emoji?: string | null;
  eligible: boolean;
  disabled: boolean;
  blockedByAllowlist: boolean;
  source: string;
  bundled: boolean;
  homepage?: string | null;
  missing: SkillRequirementState;
  installHints: SkillInstallHint[];
  managedInstalled: boolean;
  managedVersion?: string | null;
  managedPath?: string | null;
}

export interface SkillsDashboardSummary {
  managedCount: number;
  bundledCount: number;
  workspaceCount: number;
  eligibleCount: number;
  missingRequirementCount: number;
}

export interface SkillsDashboardSnapshot {
  workspaceDir: string;
  managedSkillsDir: string;
  managedSkills: SkillInfo[];
  openclawSkills: OpenClawSkillInfo[];
  summary: SkillsDashboardSummary;
  warnings: string[];
}

export interface SkillMarketplaceEntry {
  slug: string;
  displayName: string;
  summary: string;
  version?: string | null;
  updatedAt?: number | null;
  marketplace: string;
  marketplaceLabel: string;
}

export interface AgentInfo {
  id: string;
  name: string;
  model: string;
  description: string;
  path: string;
  workspace: string;
  agentDir: string;
  bindings: string[];
  skills: string[];
}

export interface ModelEntry {
  id: string;
  name: string;
  reasoning: boolean;
  input: string[];
  context_window: number;
  max_tokens: number;
}

export interface ProviderInfo {
  name: string;
  base_url: string;
  api_key: string;
  models: ModelEntry[];
}

export interface LogEntry {
  timestamp: Date;
  level: "info" | "warn" | "error" | "success";
  message: string;
}

export interface ChannelAccount {
  channel: string;
  account: string;
  name?: string;
  status?: string;
}
