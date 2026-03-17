import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Key, ArrowRight, Eye, EyeOff, Server, Globe,
  CheckCircle2, XCircle, Loader2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import type { InstallConfig, CommandResult, InstallMethod, SystemInfo } from "../types";

interface ConfigureStepProps {
  onNext: () => void;
  config: InstallConfig;
  systemInfo: SystemInfo | null;
  onConfigChange: (config: InstallConfig) => void;
}

const inputCls = "w-full bg-white/[0.03] border border-white/[0.08] rounded-lg px-2.5 py-1.5 text-[13px] placeholder-muted-foreground/50 focus:outline-none focus:border-primary/40 focus:ring-1 focus:ring-primary/20 transition-all font-mono text-foreground";

const PROVIDERS = [
  { id: "anthropic", name: "Anthropic (Claude)", placeholder: "sk-ant-...", defaultBaseUrl: "https://api.anthropic.com" },
  { id: "openai", name: "OpenAI (GPT)", placeholder: "sk-...", defaultBaseUrl: "https://api.openai.com" },
  { id: "google", name: "Google (Gemini)", placeholder: "AIza...", defaultBaseUrl: "https://generativelanguage.googleapis.com" },
  { id: "custom", name: "自定义 / OpenAI 兼容", placeholder: "your-api-key", defaultBaseUrl: "" },
];

const INSTALL_METHODS: Array<{
  id: InstallMethod;
  title: string;
  description: string;
  note: string;
}> = [
  {
    id: "npm_mirror",
    title: "npm 全局安装",
    description: "通过 npm 全局安装 openclaw，安装源使用国内淘宝镜像（npmmirror），更适合国内网络环境。",
    note: "需要本机已安装 Node.js 和 npm。",
  },
  {
    id: "official_script",
    title: "官方安装脚本",
    description: "按官方 install.sh / install.ps1 流程安装，适合已有科学上网环境或需要官方自动处理依赖的场景。",
    note: "脚本会在线拉取官方安装器。",
  },
];

type ValidationState = "idle" | "validating" | "valid" | "invalid";

export default function ConfigureStep({ onNext, config, systemInfo, onConfigChange }: ConfigureStepProps) {
  const [showKey, setShowKey] = useState(false);
  const [validation, setValidation] = useState<ValidationState>("idle");
  const [validationMsg, setValidationMsg] = useState("");
  const openclawHome = systemInfo?.openclaw_home_path || "~/.openclaw";

  const selectedProvider = PROVIDERS.find((p) => p.id === config.apiProvider) ?? PROVIDERS[0];
  const selectedInstallMethod = INSTALL_METHODS.find((method) => method.id === config.installMethod) ?? INSTALL_METHODS[0];
  const npmAvailable = Boolean(systemInfo?.npm_version);
  const installMethodBlocked = config.installMethod === "npm_mirror" && !npmAvailable;
  const customNeedsBaseUrl =
    config.apiProvider === "custom"
    && config.apiKey.trim().length > 0
    && config.apiBaseUrl.trim().length === 0;
  const customNeedsModelId =
    config.apiProvider === "custom"
    && config.apiKey.trim().length > 0
    && config.customModelId.trim().length === 0;
  const canContinue = !customNeedsBaseUrl && !customNeedsModelId && !installMethodBlocked;

  const update = (patch: Partial<InstallConfig>) => {
    // Reset validation when key or provider changes
    if (patch.apiKey !== undefined || patch.apiProvider !== undefined || patch.apiBaseUrl !== undefined) {
      setValidation("idle");
      setValidationMsg("");
      onConfigChange({ ...config, ...patch, apiKeyValidated: false });
    } else {
      onConfigChange({ ...config, ...patch });
    }
  };

  const handleValidate = async () => {
    if (!config.apiKey.trim()) return;
    setValidation("validating");
    setValidationMsg("");
    try {
      const result: CommandResult = await invoke("validate_api_key", {
        provider: config.apiProvider,
        apiKey: config.apiKey,
        baseUrl: config.apiBaseUrl || null,
      });
      if (result.success) {
        setValidation("valid");
        setValidationMsg("API Key 验证通过");
        onConfigChange({ ...config, apiKeyValidated: true });
      } else {
        setValidation("invalid");
        setValidationMsg(result.stderr || "API Key 验证失败");
        onConfigChange({ ...config, apiKeyValidated: false });
      }
    } catch (e) {
      setValidation("invalid");
      setValidationMsg(`验证出错: ${e}`);
    }
  };

  return (
    <div className="flex-1 flex flex-col p-6 animate-fade-in overflow-hidden">
      <div className="mb-4">
        <h2 className="text-lg font-semibold mb-1">安装配置</h2>
        <p className="text-[13px] text-muted-foreground">配置 AI 模型和网关参数，安装时将自动应用这些设置</p>
      </div>

      <div className="flex-1 space-y-4 overflow-y-auto pr-1">
        <Card>
          <CardContent className="p-4">
            <div className="text-[13px] font-medium mb-3">安装方式</div>
            <div className="grid gap-2">
              {INSTALL_METHODS.map((method) => (
                <button
                  key={method.id}
                  type="button"
                  onClick={() => update({ installMethod: method.id })}
                  className={`rounded-lg border p-3 text-left transition-all ${
                    config.installMethod === method.id
                      ? "border-teal-500/25 bg-teal-500/5 ring-1 ring-teal-500/15"
                      : "border-white/[0.06] bg-white/[0.02] hover:border-teal-500/15"
                  }`}
                >
                  <div className="flex items-center justify-between gap-3">
                    <div className="text-[12px] font-medium">{method.title}</div>
                    {method.id === "npm_mirror" && (
                      <span className="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] text-emerald-400">
                        推荐
                      </span>
                    )}
                  </div>
                  <p className="mt-1 text-[11px] text-muted-foreground leading-relaxed">{method.description}</p>
                  <p className="mt-2 text-[10px] text-muted-foreground/70">{method.note}</p>
                </button>
              ))}
            </div>

            <div className={`mt-3 rounded-lg border px-3 py-2 text-[11px] ${
              config.installMethod === "npm_mirror" && !npmAvailable
                ? "border-amber-500/20 bg-amber-500/5 text-amber-300"
                : "border-white/[0.06] bg-white/[0.02] text-muted-foreground"
            }`}>
              {config.installMethod === "npm_mirror"
                ? npmAvailable
                  ? `已检测到 npm v${systemInfo?.npm_version}，将使用 npmmirror 国内镜像执行全局安装。`
                  : "当前未检测到 npm，国内镜像安装方式暂不可用。你可以先安装 Node.js / npm，或者切换到官方脚本。"
                : "将使用官方在线安装脚本；如果当前网络无法访问官方源，建议改用上面的 npm 国内镜像方式。"}
            </div>
          </CardContent>
        </Card>

        <Card className="border-white/[0.06] bg-white/[0.02]">
          <CardContent className="p-4">
            <div className="text-[12px] font-medium text-foreground/90">这一步可以先简配</div>
            <p className="mt-1 text-[11px] text-muted-foreground leading-relaxed">
              如果你只是想先把 OpenClaw 装起来，可以暂时不填 API Key，后面进入控制面板后再去模型管理补配置。
            </p>
          </CardContent>
        </Card>

        {/* API Provider */}
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-2 mb-3">
              <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-teal-500/10">
                <Key size={13} className="text-teal-400" />
              </div>
              <span className="text-[13px] font-medium">AI 模型提供商</span>
            </div>

            <div className="grid grid-cols-2 gap-2 mb-3">
              {PROVIDERS.map((provider) => (
                <button
                  key={provider.id}
                  onClick={() => update({
                    apiProvider: provider.id,
                    apiBaseUrl: provider.id === "custom" ? config.apiBaseUrl : provider.defaultBaseUrl,
                  })}
                  className={`p-2.5 rounded-lg border text-left transition-all text-[12px] ${
                    config.apiProvider === provider.id
                      ? "bg-teal-500/5 border-teal-500/25 ring-1 ring-teal-500/15"
                      : "bg-white/[0.02] border-white/[0.06] hover:border-teal-500/15"
                  }`}
                >
                  {provider.name}
                </button>
              ))}
            </div>

            <div className="space-y-2.5">
              <div>
                <label className="text-[11px] text-muted-foreground block mb-1">API Key</label>
                <div className="relative">
                  <input
                    type={showKey ? "text" : "password"}
                    value={config.apiKey}
                    onChange={(e) => update({ apiKey: e.target.value })}
                    placeholder={selectedProvider.placeholder}
                    className={`${inputCls} pr-8`}
                  />
                  <button
                    onClick={() => setShowKey(!showKey)}
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground transition-colors"
                  >
                    {showKey ? <EyeOff size={13} /> : <Eye size={13} />}
                  </button>
                </div>
                {/* Validation button & feedback */}
                <div className="flex items-center gap-2 mt-2">
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={handleValidate}
                    disabled={!config.apiKey.trim() || validation === "validating"}
                    className="text-[11px] h-7"
                  >
                    {validation === "validating" && <Loader2 size={11} className="animate-spin mr-1" />}
                    {validation === "valid" && <CheckCircle2 size={11} className="text-emerald-400 mr-1" />}
                    {validation === "invalid" && <XCircle size={11} className="text-red-400 mr-1" />}
                    验证 Key
                  </Button>
                  {validationMsg && (
                    <span className={`text-[11px] ${validation === "valid" ? "text-emerald-400" : "text-red-400"}`}>
                      {validationMsg}
                    </span>
                  )}
                </div>
              </div>

              {config.apiProvider === "custom" && (
                <>
                  <div>
                    <label className="text-[11px] text-muted-foreground block mb-1">
                      <Globe size={11} className="inline mr-1" />
                      API Base URL
                    </label>
                    <input
                      type="text"
                      value={config.apiBaseUrl}
                      onChange={(e) => update({ apiBaseUrl: e.target.value })}
                      placeholder="https://api.example.com/v1"
                      className={inputCls}
                    />
                    <p className="mt-1 text-[10px] text-muted-foreground/60">
                      自定义 / OpenAI 兼容接口需要填写完整 Base URL，例如 `https://host/v1`
                    </p>
                  </div>

                  <div>
                    <label className="text-[11px] text-muted-foreground block mb-1">
                      默认模型 ID
                    </label>
                    <input
                      type="text"
                      value={config.customModelId}
                      onChange={(e) => update({ customModelId: e.target.value })}
                      placeholder="gpt-4o-mini / foo-large"
                      className={inputCls}
                    />
                    <p className="mt-1 text-[10px] text-muted-foreground/60">
                      官方非交互式 `openclaw onboard` 对自定义 Provider 需要 `--custom-model-id`
                    </p>
                  </div>
                </>
              )}
            </div>

            <p className="text-[10px] text-muted-foreground/50 mt-2">
              API Key 仅保存在本地 {openclawHome} 目录，不会上传
            </p>
          </CardContent>
        </Card>

        {/* Gateway */}
        <Card>
          <CardContent className="p-4">
            <div className="flex items-center gap-2 mb-3">
              <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-sky-500/10">
                <Server size={13} className="text-sky-400" />
              </div>
              <span className="text-[13px] font-medium">网关端口</span>
            </div>
            <div className="flex items-center gap-2">
              <input
                type="number"
                value={config.gatewayPort}
                onChange={(e) => update({ gatewayPort: parseInt(e.target.value) || 18789 })}
                className={`${inputCls} w-32`}
                min={1024}
                max={65535}
              />
              <span className="text-[11px] text-muted-foreground">默认 18789</span>
            </div>
          </CardContent>
        </Card>
      </div>

      <div className="mt-4 flex items-center justify-between">
        <p className="text-[11px] text-muted-foreground">
          {installMethodBlocked
            ? "当前未检测到 npm，请先安装 Node.js / npm 或切换到官方脚本"
            : customNeedsBaseUrl
            ? "自定义 Provider 需要填写 Base URL 才能继续"
            : customNeedsModelId
            ? "自定义 Provider 需要填写模型 ID 才能完成官方 onboard"
            : config.apiKey
              ? `${selectedInstallMethod.title}已就绪，点击下一步开始安装`
              : `将使用${selectedInstallMethod.title}安装，模型配置也可以稍后再补`}
        </p>
        <Button onClick={onNext} disabled={!canContinue}>
          {config.apiKey ? "下一步" : "跳过配置"} <ArrowRight size={14} />
        </Button>
      </div>
    </div>
  );
}
