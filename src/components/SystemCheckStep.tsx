import { useEffect, useState } from "react";
import {
  CheckCircle2, XCircle, Loader2, Monitor, Cpu, HardDrive,
  Package, ArrowRight, GitBranch, FolderOpen, AlertTriangle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import type { SystemInfo } from "../types";

interface SystemCheckStepProps {
  onNext: () => void;
  systemInfo: SystemInfo;
}

type CheckStatus = "pending" | "checking" | "pass" | "fail" | "warn";

interface CheckItem {
  id: string;
  label: string;
  description: string;
  icon: typeof Monitor;
  status: CheckStatus;
  detail?: string;
}

const INITIAL_CHECKS: CheckItem[] = [
  { id: "os", label: "操作系统", description: "macOS 11+ / Linux / Windows 10+", icon: Monitor, status: "pending" },
  { id: "node", label: "Node.js", description: "需要 v22 或更高版本", icon: Package, status: "pending" },
  { id: "npm", label: "npm", description: "Node.js 自带的包管理器", icon: Package, status: "pending" },
  { id: "git", label: "Git", description: "版本控制工具", icon: GitBranch, status: "pending" },
  { id: "memory", label: "内存", description: "最低 4GB，推荐 8GB+", icon: Cpu, status: "pending" },
  { id: "disk", label: "磁盘空间", description: "至少 1GB 可用空间", icon: HardDrive, status: "pending" },
  { id: "oc-cli", label: "OpenClaw CLI", description: "openclaw 命令是否可用", icon: Package, status: "pending" },
  { id: "oc-home", label: "安装目录", description: "OpenClaw 数据目录", icon: FolderOpen, status: "pending" },
  { id: "oc-config", label: "配置文件", description: "openclaw.json 配置文件", icon: FolderOpen, status: "pending" },
  { id: "oc-doctor", label: "健康检查", description: "openclaw doctor 验证", icon: FolderOpen, status: "pending" },
];

export default function SystemCheckStep({ onNext, systemInfo }: SystemCheckStepProps) {
  const [checks, setChecks] = useState<CheckItem[]>(INITIAL_CHECKS);
  const [allDone, setAllDone] = useState(false);
  const [canProceed, setCanProceed] = useState(false);
  const [tips, setTips] = useState<string[]>([]);
  const homePath = systemInfo.openclaw_home_path || "~/.openclaw";
  const hasExistingInstall = systemInfo.openclaw_cli_ok || systemInfo.openclaw_home_exists || systemInfo.openclaw_config_exists;
  const repairMode = hasExistingInstall && !systemInfo.openclaw_fully_installed;

  const updateCheck = (id: string, status: CheckStatus, detail?: string) => {
    setChecks((prev) => prev.map((c) => (c.id === id ? { ...c, status, detail } : c)));
  };

  useEffect(() => {
    let cancelled = false;
    const info = systemInfo;

    async function showResults() {
      // Animate through checks using the already-fetched systemInfo
      for (const check of INITIAL_CHECKS) {
        if (cancelled) return;
        updateCheck(check.id, "checking");
        await delay(60);
      }

      if (cancelled) return;

      const newTips: string[] = [];
      const osLabel = { macos: "macOS", linux: "Linux", windows: "Windows" }[info.os] ?? info.os;

      updateCheck("os", "pass", `${osLabel} (${info.arch})`);
      await delay(60);

      if (info.node_version && info.node_ok) {
        updateCheck("node", "pass", info.node_version);
      } else if (info.node_version) {
        updateCheck("node", "warn", `${info.node_version}（需要 v22+，安装脚本将自动处理）`);
      } else {
        updateCheck("node", "warn", "未安装（安装脚本将自动安装）");
      }
      await delay(60);

      if (info.npm_version) updateCheck("npm", "pass", `v${info.npm_version}`);
      else updateCheck("npm", "warn", "未检测到（安装脚本将自动处理）");
      await delay(60);

      if (info.git_version) updateCheck("git", "pass", `v${info.git_version}`);
      else {
        updateCheck("git", "warn", "未安装");
        if (info.os === "macos") newTips.push("安装 Git: xcode-select --install");
        else if (info.os === "linux") newTips.push("安装 Git: sudo apt install -y git");
        else newTips.push("安装 Git: https://git-scm.com/download/win");
      }
      await delay(60);

      const memStr = `${info.total_memory_gb.toFixed(1)} GB`;
      if (info.memory_recommended) updateCheck("memory", "pass", memStr);
      else if (info.memory_ok) updateCheck("memory", "warn", `${memStr}（推荐 8GB+）`);
      else updateCheck("memory", "fail", `${memStr}（最低 4GB）`);
      await delay(60);

      const diskStr = `${info.free_disk_gb.toFixed(1)} GB 可用`;
      if (info.disk_ok) updateCheck("disk", "pass", diskStr);
      else updateCheck("disk", "fail", `${diskStr}（至少需要 1GB）`);
      await delay(60);

      if (info.openclaw_cli_ok && info.openclaw_version) updateCheck("oc-cli", "pass", info.openclaw_version);
      else if (info.openclaw_version) updateCheck("oc-cli", "warn", `${info.openclaw_version}（命令异常）`);
      else updateCheck("oc-cli", "warn", "未找到 openclaw 命令");
      await delay(60);

      if (info.openclaw_home_exists) updateCheck("oc-home", "pass", `${homePath} 存在`);
      else updateCheck("oc-home", "warn", "目录不存在（将在安装时创建）");
      await delay(60);

      if (info.openclaw_config_exists) updateCheck("oc-config", "pass", "已找到配置文件");
      else updateCheck("oc-config", "warn", "配置文件不存在（安装后自动生成）");
      await delay(60);

      if (info.openclaw_doctor_ok) updateCheck("oc-doctor", "pass", "健康检查通过");
      else if (info.openclaw_cli_ok) updateCheck("oc-doctor", "warn", "健康检查未通过");
      else updateCheck("oc-doctor", "warn", "CLI 不可用，跳过");

      if (repairMode) {
        newTips.unshift("已检测到本机已有 OpenClaw，下一步会补全配置并重新验证可用性");
      }

      setTips(newTips);
      setCanProceed(info.disk_ok && info.memory_ok);
      setAllDone(true);
    }

    showResults();
    return () => { cancelled = true; };
  }, [homePath, systemInfo]);

  const statusIcon = (status: CheckStatus) => {
    switch (status) {
      case "pending": return <div className="w-4 h-4 rounded-full border-2 border-white/10" />;
      case "checking": return <Loader2 size={16} className="text-teal-400 animate-spin" />;
      case "pass": return <CheckCircle2 size={16} className="text-emerald-400" />;
      case "fail": return <XCircle size={16} className="text-red-400" />;
      case "warn": return <AlertTriangle size={16} className="text-amber-400" />;
    }
  };

  return (
    <div className="flex-1 flex flex-col p-6 animate-fade-in overflow-hidden">
      <div className="mb-4">
        <h2 className="text-lg font-semibold mb-1">环境检测</h2>
        <p className="text-[13px] text-muted-foreground">
          {repairMode
            ? "检测到这台机器上已经有 OpenClaw 痕迹，先确认环境后再补全安装"
            : "正在检查系统是否满足 OpenClaw 的运行要求"}
        </p>
      </div>

      <Card className={`mb-3 ${repairMode ? "border-amber-500/15 bg-amber-500/5" : "border-teal-500/15 bg-teal-500/5"}`}>
        <CardContent className="p-3">
          <div className="text-[12px] font-medium mb-1">
            {repairMode ? "本次会走修复安装流程" : "本次会走全新安装流程"}
          </div>
          <p className="text-[11px] text-muted-foreground leading-relaxed">
            {repairMode
              ? "不会把你本地现有 OpenClaw 当成全新环境重来一遍，而是继续补配置、跑健康检查并确认能正常使用。"
              : "缺少的依赖会在安装阶段按官方脚本自动处理，你只需要关注磁盘和基础环境是否足够。"}
          </p>
        </CardContent>
      </Card>

      <div className="flex-1 space-y-1.5 overflow-y-auto pr-1">
        {checks.map((check) => {
          const Icon = check.icon;
          return (
            <div
              key={check.id}
              className={`flex items-center gap-3 px-3 py-2 rounded-lg border transition-all duration-300 ${
                check.status === "checking" ? "bg-teal-500/5 border-teal-500/20"
                  : check.status === "fail" ? "bg-red-500/5 border-red-500/15"
                  : check.status === "pass" ? "bg-emerald-500/5 border-emerald-500/10"
                  : check.status === "warn" ? "bg-amber-500/5 border-amber-500/10"
                  : "bg-white/[0.02] border-white/[0.06]"
              }`}
            >
              <div className="w-7 h-7 rounded-md bg-white/[0.04] flex items-center justify-center shrink-0">
                <Icon size={13} className="text-muted-foreground" />
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-[13px] font-medium">{check.label}</span>
                  {check.detail && <span className="text-[11px] text-muted-foreground truncate">— {check.detail}</span>}
                </div>
                <p className="text-[11px] text-muted-foreground mt-0.5">{check.description}</p>
              </div>
              <div className="shrink-0">{statusIcon(check.status)}</div>
            </div>
          );
        })}
      </div>

      {allDone && tips.length > 0 && (
        <Card className="mt-3 border-amber-500/10 bg-amber-500/5">
          <CardContent className="p-3">
            <div className="text-[11px] font-medium text-amber-400 mb-1.5">修复建议</div>
            <div className="space-y-1">
              {tips.map((tip, i) => (
                <code key={i} className="block text-[11px] text-muted-foreground bg-white/[0.03] px-2 py-1 rounded-md font-mono">{tip}</code>
              ))}
            </div>
          </CardContent>
        </Card>
      )}

      {allDone && (
        <div className="mt-3 flex items-center justify-between animate-fade-in">
          <div className="text-[13px]">
            {canProceed
              ? <span className="text-emerald-400">{repairMode ? "环境检测通过，可以继续修复安装" : "环境检测通过，可以继续安装"}</span>
              : <span className="text-red-400">内存或磁盘空间不足，建议先处理后再继续</span>
            }
          </div>
          <Button onClick={onNext} disabled={!canProceed}>
            {repairMode ? "继续修复安装" : "继续安装"} <ArrowRight size={14} />
          </Button>
        </div>
      )}
    </div>
  );
}

function delay(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}
