import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Loader2, CheckCircle2, XCircle, Circle } from "lucide-react";
import type { SystemInfo } from "@/types";

interface LoadingScreenProps {
  onResult: (info: SystemInfo) => void;
}

interface DetectItem {
  label: string;
  status: "pending" | "checking" | "pass" | "fail";
}

const CHECK_STAGGER_MS = 80;

export default function LoadingScreen({ onResult }: LoadingScreenProps) {
  const [items, setItems] = useState<DetectItem[]>([
    { label: "系统环境", status: "pending" },
    { label: "OpenClaw CLI", status: "pending" },
    { label: "安装目录", status: "pending" },
    { label: "配置文件", status: "pending" },
    { label: "健康检查 (openclaw doctor)", status: "pending" },
  ]);

  const updateItem = (idx: number, status: DetectItem["status"]) => {
    setItems((prev) => prev.map((it, i) => (i === idx ? { ...it, status } : it)));
  };

  useEffect(() => {
    let cancelled = false;

    async function detect() {
      updateItem(0, "checking");

      let info: SystemInfo;
      try {
        info = await invoke("check_system");
      } catch {
        if (cancelled) return;
        updateItem(0, "fail");
        await delay(200);
        onResult(fallbackInfo());
        return;
      }

      if (cancelled) return;
      updateItem(0, "pass");
      await delay(CHECK_STAGGER_MS);

      updateItem(1, "checking"); await delay(CHECK_STAGGER_MS);
      updateItem(1, info.openclaw_cli_ok ? "pass" : "fail"); await delay(CHECK_STAGGER_MS);

      updateItem(2, "checking"); await delay(CHECK_STAGGER_MS);
      updateItem(2, info.openclaw_home_exists ? "pass" : "fail"); await delay(CHECK_STAGGER_MS);

      updateItem(3, "checking"); await delay(CHECK_STAGGER_MS);
      updateItem(3, info.openclaw_config_exists ? "pass" : "fail"); await delay(CHECK_STAGGER_MS);

      updateItem(4, "checking"); await delay(CHECK_STAGGER_MS);
      updateItem(4, info.openclaw_doctor_ok ? "pass" : "fail"); await delay(160);

      if (!cancelled) onResult(info);
    }

    detect();
    return () => { cancelled = true; };
  }, [onResult]);

  return (
    <div className="flex h-screen flex-col items-center justify-center bg-background">
      <img src="/icon.png" alt="OpenClaw" className="mb-6 h-14 w-14 rounded-xl shadow-lg shadow-teal-500/25" />

      <div className="w-72 rounded-xl border border-white/[0.06] bg-card p-4 shadow-lg shadow-black/10 space-y-2.5">
        {items.map((item, i) => (
          <div key={i} className="flex items-center gap-3">
            <div className="flex h-5 w-5 shrink-0 items-center justify-center">
              {item.status === "pending" && <Circle size={8} className="text-muted-foreground/30" />}
              {item.status === "checking" && <Loader2 size={14} className="animate-spin text-teal-400" />}
              {item.status === "pass" && <CheckCircle2 size={14} className="text-emerald-400" />}
              {item.status === "fail" && <XCircle size={14} className="text-muted-foreground" />}
            </div>
            <span className={`text-[13px] ${
              item.status === "checking" ? "text-teal-400 font-medium"
                : item.status === "pass" ? "text-foreground"
                : "text-muted-foreground"
            }`}>
              {item.label}
            </span>
          </div>
        ))}
      </div>

      <p className="mt-4 text-xs text-muted-foreground">正在检测系统环境...</p>
    </div>
  );
}

function delay(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

function fallbackInfo(): SystemInfo {
  return {
    os: "unknown", arch: "unknown",
    node_version: null, npm_version: null, pnpm_version: null, git_version: null, openclaw_version: null,
    total_memory_gb: 0, free_disk_gb: 0,
    openclaw_home_exists: false, openclaw_home_path: "", openclaw_config_exists: false, openclaw_config_path: null, openclaw_cli_ok: false,
    openclaw_doctor_ok: false, openclaw_fully_installed: false, gateway_port: null,
    node_ok: false, memory_ok: false, memory_recommended: false, disk_ok: false,
  };
}
