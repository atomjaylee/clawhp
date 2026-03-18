import { useState, useCallback } from "react";
import { SidebarInset, SidebarProvider, SidebarTrigger } from "@/components/ui/sidebar";
import { Separator } from "@/components/ui/separator";
import { Breadcrumb, BreadcrumbItem, BreadcrumbList, BreadcrumbPage } from "@/components/ui/breadcrumb";
import { Badge } from "@/components/ui/badge";
import { AppSidebar } from "@/components/app-sidebar";
import LoadingScreen from "@/components/LoadingScreen";
import DashboardContent from "@/components/DashboardContent";
import SkillsPage from "@/components/SkillsPage";
import AgentsPage from "@/components/AgentsPage";
import ModelsPage from "@/components/ModelsPage";
import ChannelsPage from "@/components/ChannelsPage";
import SettingsPage from "@/components/SettingsPage";
import WelcomeStep from "@/components/WelcomeStep";
import SystemCheckStep from "@/components/SystemCheckStep";
import InstallStep from "@/components/InstallStep";
import ConfigureStep from "@/components/ConfigureStep";
import type { AppMode, WizardStep, DashboardTab, SystemInfo, InstallConfig } from "@/types";

const WIZARD_STEPS: WizardStep[] = ["welcome", "check", "configure", "install"];

const PAGE_TITLES: Record<DashboardTab, string> = {
  dashboard: "仪表盘",
  channels: "频道",
  skills: "Skills",
  agents: "Agents",
  models: "模型管理",
  settings: "设置",
};

const WIZARD_TITLES: Record<WizardStep, string> = {
  welcome: "欢迎",
  check: "环境检测",
  configure: "安装配置",
  install: "安装 OpenClaw",
};

export default function App() {
  const [mode, setMode] = useState<AppMode>("loading");
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null);
  const [wizardStep, setWizardStep] = useState<WizardStep>("welcome");
  const [completedSteps, setCompletedSteps] = useState<Set<WizardStep>>(new Set());
  const [dashboardTab, setDashboardTab] = useState<DashboardTab>("dashboard");
  const [installConfig, setInstallConfig] = useState<InstallConfig>({
    apiProvider: "anthropic",
    apiKey: "",
    apiBaseUrl: "",
    customModelId: "",
    gatewayPort: 18789,
    installMethod: "npm_mirror",
  });

  const handleDetectionResult = useCallback((info: SystemInfo) => {
    setSystemInfo(info);
    const hasUsableExistingInstall =
      info.openclaw_fully_installed || (info.openclaw_cli_ok && info.openclaw_config_exists);

    if (hasUsableExistingInstall) {
      setMode("dashboard");
    } else {
      const nextStep: WizardStep = info.openclaw_cli_ok ? "check" : "welcome";
      setWizardStep(nextStep);
      setCompletedSteps(nextStep === "check" ? new Set<WizardStep>(["welcome"]) : new Set());
      setMode("wizard");
    }
  }, []);

  const goNextWizard = useCallback(() => {
    const idx = WIZARD_STEPS.indexOf(wizardStep);
    if (idx < WIZARD_STEPS.length - 1) {
      setCompletedSteps((prev) => new Set([...prev, wizardStep]));
      setWizardStep(WIZARD_STEPS[idx + 1]);
    }
  }, [wizardStep]);

  const handleSkipToDashboard = useCallback(() => {
    setMode("loading");
  }, []);

  const handleWizardNavigate = useCallback((step: WizardStep) => {
    const targetIdx = WIZARD_STEPS.indexOf(step);
    const currentIdx = WIZARD_STEPS.indexOf(wizardStep);
    if (targetIdx <= currentIdx || completedSteps.has(step)) {
      setWizardStep(step);
    }
  }, [wizardStep, completedSteps]);

  const handleWizardComplete = useCallback(() => {
    setDashboardTab("dashboard");
    setMode("dashboard");
  }, []);

  const handleInstallVerified = useCallback((info: SystemInfo) => {
    setSystemInfo(info);
  }, []);

  const handleSystemInfoRefresh = useCallback((info: SystemInfo) => {
    setSystemInfo(info);
  }, []);

  if (mode === "loading") {
    return <LoadingScreen onResult={handleDetectionResult} />;
  }

  const pageTitle = mode === "dashboard"
    ? PAGE_TITLES[dashboardTab]
    : WIZARD_TITLES[wizardStep];

  const renderContent = () => {
    if (mode === "dashboard" && systemInfo) {
      switch (dashboardTab) {
        case "dashboard":
          return <DashboardContent systemInfo={systemInfo} onNavigate={setDashboardTab} />;
        case "skills":
          return <SkillsPage />;
        case "agents":
          return <AgentsPage />;
        case "models":
          return <ModelsPage />;
        case "channels":
          return <ChannelsPage />;
        case "settings":
          return (
            <SettingsPage
              systemInfo={systemInfo}
              onSystemInfoRefresh={handleSystemInfoRefresh}
              onUninstallComplete={() => { setMode("loading"); }}
            />
          );
      }
    }
    switch (wizardStep) {
      case "welcome": return <WelcomeStep onNext={goNextWizard} onSkip={handleSkipToDashboard} />;
      case "check": return <SystemCheckStep onNext={goNextWizard} systemInfo={systemInfo!} />;
      case "configure": return <ConfigureStep onNext={goNextWizard} config={installConfig} systemInfo={systemInfo} onConfigChange={setInstallConfig} />;
      case "install": return <InstallStep onNext={handleWizardComplete} onInstalled={handleInstallVerified} systemInfo={systemInfo} config={installConfig} />;
    }
  };

  return (
    <div className="h-full w-full overflow-hidden bg-background">
      <SidebarProvider className="h-full min-h-0">
        <AppSidebar
          mode={mode}
          activeTab={dashboardTab}
          onTabChange={setDashboardTab}
          wizardStep={wizardStep}
          onWizardNavigate={handleWizardNavigate}
          completedSteps={completedSteps}
          version={systemInfo?.openclaw_version ?? undefined}
        />
        <SidebarInset className="min-h-0 overflow-hidden">
          <div className="h-6 shrink-0 border-b border-white/[0.05] bg-background/90" data-tauri-drag-region />
          <header className="flex h-9 shrink-0 items-center gap-2 border-b border-white/[0.06] bg-background/95 px-2.5">
            <div className="flex min-w-0 items-center gap-2">
              <SidebarTrigger className="-ml-0.5 text-muted-foreground hover:text-foreground" />
              <Separator orientation="vertical" className="h-4 bg-white/[0.06]" />
              <Breadcrumb className="min-w-0">
                <BreadcrumbList>
                  <BreadcrumbItem className="min-w-0">
                    <BreadcrumbPage className="truncate text-[12px] text-muted-foreground">{pageTitle}</BreadcrumbPage>
                  </BreadcrumbItem>
                </BreadcrumbList>
              </Breadcrumb>
            </div>
            <div className="h-full flex-1" data-tauri-drag-region />
            <Badge variant="outline" className="h-6 border-white/[0.08] bg-white/[0.03] px-2 text-[10px] text-muted-foreground">
              {mode === "dashboard" ? "控制台" : "安装向导"}
            </Badge>
          </header>
          <div className="flex min-h-0 flex-1 overflow-hidden">
            {renderContent()}
          </div>
        </SidebarInset>
      </SidebarProvider>
    </div>
  );
}
