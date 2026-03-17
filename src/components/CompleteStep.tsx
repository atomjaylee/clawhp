import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  PartyPopper, ExternalLink, Terminal, BookOpen,
  RotateCcw, Stethoscope, RefreshCw, ArrowRight,
  CheckCircle2, XCircle, Loader2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import type { CommandResult, SkillInfo, ProviderInfo } from "../types";

interface CompleteStepProps {
  onRestart: () => void;
  onEnterDashboard: () => void;
  gatewayPort: number;
}

interface LiveStatus {
  gatewayRunning: boolean | null;
  gatewayState: string | null;
  providerName: string | null;
  skillCount: number | null;
  doctorOk: boolean | null;
  loading: boolean;
}

interface GatewayStatusSnapshot {
  service?: {
    runtime?: {
      status?: string;
      state?: string;
    };
  };
  rpc?: {
    ok?: boolean;
  };
}

export default function CompleteStep({ onRestart, onEnterDashboard, gatewayPort }: CompleteStepProps) {
  const [enteringDashboard, setEnteringDashboard] = useState(false);
  const [status, setStatus] = useState<LiveStatus>({
    gatewayRunning: null, gatewayState: null, providerName: null, skillCount: null, doctorOk: null, loading: true,
  });

  const checkAll = useCallback(async () => {
    setStatus((s) => ({ ...s, loading: true }));
    try {
      const [gw, portCheck, providers, skills, doctor] = await Promise.allSettled([
        invoke<CommandResult>("get_gateway_status_snapshot"),
        invoke<CommandResult>("check_gateway_port", { port: gatewayPort }),
        invoke<ProviderInfo[]>("list_providers"),
        invoke<SkillInfo[]>("list_skills"),
        invoke<CommandResult>("run_openclaw_command", { args: ["doctor"] }),
      ]);

      const gatewaySnapshot = gw.status === "fulfilled"
        ? parseJsonResult<GatewayStatusSnapshot>(gw.value)
        : null;
      const gatewayState = gatewaySnapshot?.service?.runtime?.status
        ?? gatewaySnapshot?.service?.runtime?.state
        ?? null;
      const gatewayRunning = gatewayState === "running"
        || gatewayState === "active"
        || (gatewaySnapshot?.rpc?.ok ?? false)
        || (portCheck.status === "fulfilled" && portCheck.value.success);

      setStatus({
        gatewayRunning,
        gatewayState,
        providerName: providers.status === "fulfilled" && providers.value.length > 0
          ? providers.value[0].name : null,
        skillCount: skills.status === "fulfilled" ? skills.value.length : null,
        doctorOk: doctor.status === "fulfilled" ? doctor.value.success : false,
        loading: false,
      });
    } catch {
      setStatus((s) => ({ ...s, loading: false }));
    }
  }, [gatewayPort]);

  useEffect(() => { checkAll(); }, [checkAll]);
  return (
    <div className="flex-1 flex flex-col items-center justify-center p-6 animate-fade-in overflow-auto">
      <div className="max-w-lg w-full text-center">
        <div className="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-xl bg-gradient-to-br from-emerald-400 to-emerald-600 shadow-lg shadow-emerald-500/25">
          <PartyPopper size={26} className="text-white" />
        </div>

        <h2 className="text-xl font-bold mb-2">安装完成</h2>
        <p className="text-[13px] text-muted-foreground mb-6 leading-relaxed">
          OpenClaw 已成功安装并配置完毕。<br />
          进入控制面板启动网关、管理模型和技能。
        </p>

        {/* Live Status Indicators */}
        <div className="grid grid-cols-2 gap-2 mb-6">
          <StatusBadge
            label="网关"
            loading={status.loading}
            ok={status.gatewayRunning}
            okText={status.gatewayState ?? "运行中"}
            failText="未启动"
          />
          <StatusBadge
            label="提供商"
            loading={status.loading}
            ok={status.providerName !== null}
            okText={status.providerName ?? ""}
            failText="未配置"
          />
          <StatusBadge
            label="技能"
            loading={status.loading}
            ok={status.skillCount !== null && status.skillCount > 0}
            okText={`${status.skillCount} 个已安装`}
            failText="无"
          />
          <StatusBadge
            label="Doctor"
            loading={status.loading}
            ok={status.doctorOk}
            okText="通过"
            failText="异常"
          />
        </div>

        <div className="space-y-2 mb-6 text-left">
          <h3 className="text-[13px] font-medium text-center mb-2">快速参考</h3>

          <ActionCard icon={Terminal} title="运行状态" desc="新版推荐先检查 OpenClaw 当前状态" command="openclaw status" />
          <ActionCard icon={RefreshCw} title="网关服务" desc="确认 daemon 已安装并且网关真正运行" command="openclaw gateway status" />
          <ActionCard 
            icon={BookOpen} 
            title="Control UI" 
            desc="官方推荐通过 openclaw dashboard 打开控制界面" 
            command="openclaw dashboard" 
            onClick={async () => {
              try {
                const tokenResult: CommandResult = await invoke("get_gateway_token");
                const token = tokenResult.success ? tokenResult.stdout.trim() : "";
                const controlUiUrl = token
                  ? `http://127.0.0.1:${gatewayPort}/?token=${token}`
                  : `http://127.0.0.1:${gatewayPort}/`;
                const popup = window.open(controlUiUrl, "_blank", "noopener,noreferrer");
                if (popup) {
                  return;
                }
              } catch {
                // ignore
              }
              try {
                const r: CommandResult = await invoke("open_dashboard");
                if (r.success) {
                  return;
                }
              } catch {
                // ignore
              }
              window.open(`http://127.0.0.1:${gatewayPort}/`, "_blank", "noopener,noreferrer");
            }}
          />
          <ActionCard icon={Stethoscope} title="健康检查" desc="验证安装是否正常" command="openclaw doctor" />
          <ActionCard icon={ExternalLink} title="官方文档" desc="了解配置、频道、技能等更多功能" command="https://docs.openclaw.ai/start/getting-started" isLink />
        </div>

        <div className="flex items-center justify-center gap-3">
          <Button variant="outline" onClick={onRestart}>
            <RotateCcw />
            重新检测
          </Button>
          <Button onClick={() => { setEnteringDashboard(true); onEnterDashboard(); }} disabled={enteringDashboard}>
            {enteringDashboard ? <Loader2 size={14} className="animate-spin" /> : null}
            {enteringDashboard ? "正在进入..." : "进入控制面板"}
            {!enteringDashboard && <ArrowRight size={14} />}
          </Button>
        </div>
      </div>
    </div>
  );
}

function ActionCard({ 
  icon: Icon, 
  title, 
  desc, 
  command, 
  isLink, 
  onClick 
}: { 
  icon: any; 
  title: string; 
  desc: string; 
  command: string; 
  isLink?: boolean;
  onClick?: () => void | Promise<void>;
}) {
  const [pending, setPending] = useState(false);

  const handleClick = () => {
    if (!onClick || pending) {
      return;
    }

    const result = onClick();
    if (result && typeof (result as Promise<void>).then === "function") {
      setPending(true);
      (result as Promise<void>).finally(() => {
        setPending(false);
      });
    }
  };

  const inner = (
    <Card className={`group bg-white/[0.02] border-white/[0.04] hover:bg-white/[0.04] hover:border-white/[0.08] transition-colors cursor-pointer ${isLink || onClick ? '' : ''}`} onClick={(e) => {
      if (onClick) {
        e.preventDefault();
        handleClick();
      }
    }}>
      <CardContent className="p-3">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-white/[0.05] text-muted-foreground group-hover:bg-primary/20 group-hover:text-primary transition-colors">
            <Icon size={16} />
          </div>
          <div className="flex-1 min-w-0">
            <h4 className="text-[13px] font-medium text-foreground/90 leading-none mb-1.5">{title}</h4>
            <p className="text-[11px] text-muted-foreground truncate">{desc}</p>
          </div>
          {(isLink || onClick) && (
            pending
              ? <Loader2 size={14} className="animate-spin text-muted-foreground ml-2 shrink-0" />
              : <ArrowRight size={14} className="text-muted-foreground group-hover:text-primary transition-colors ml-2 shrink-0" />
          )}
        </div>
      </CardContent>
    </Card>
  );

  if (onClick) {
    return inner;
  }
  
  if (isLink) {
    return (
      <a href={command} target="_blank" rel="noreferrer" className="block focus:outline-none focus:ring-2 focus:ring-primary focus:ring-offset-2 focus:ring-offset-background rounded-xl">
        {inner}
      </a>
    );
  }

  return inner;
}

function StatusBadge({ label, loading, ok, okText, failText }: {
  label: string; loading: boolean; ok: boolean | null; okText: string; failText: string;
}) {
  return (
    <div className="flex items-center gap-2 p-2.5 rounded-lg border border-white/[0.06] bg-white/[0.02]">
      {loading ? (
        <Loader2 size={12} className="text-muted-foreground animate-spin shrink-0" />
      ) : ok ? (
        <CheckCircle2 size={12} className="text-emerald-400 shrink-0" />
      ) : (
        <XCircle size={12} className="text-amber-400 shrink-0" />
      )}
      <div className="min-w-0">
        <div className="text-[10px] text-muted-foreground">{label}</div>
        <div className="text-[11px] font-medium truncate">
          {loading ? "检查中..." : ok ? okText : failText}
        </div>
      </div>
    </div>
  );
}

function parseJsonResult<T>(result: CommandResult): T | null {
  if (!result.success || !result.stdout) {
    return null;
  }

  try {
    return JSON.parse(result.stdout) as T;
  } catch {
    return null;
  }
}
