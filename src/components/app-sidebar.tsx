import {
  Activity,
  Puzzle,
  Bot,
  Box,
  Settings,
  Download,
  Search,
  CheckCircle2,
  MessageSquare,
  Home,
  type LucideIcon,
} from "lucide-react";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import type { AppMode, DashboardTab, WizardStep } from "@/types";

interface AppSidebarProps {
  mode: AppMode;
  activeTab?: DashboardTab;
  onTabChange?: (tab: DashboardTab) => void;
  wizardStep?: WizardStep;
  onWizardNavigate?: (step: WizardStep) => void;
  completedSteps?: Set<WizardStep>;
  appVersion?: string;
}

const dashboardNav: { id: DashboardTab; label: string; icon: LucideIcon }[] = [
  { id: "dashboard", label: "仪表盘", icon: Activity },
  { id: "channels", label: "频道", icon: MessageSquare },
  { id: "skills", label: "Skills", icon: Puzzle },
  { id: "agents", label: "Agents", icon: Bot },
  { id: "models", label: "模型管理", icon: Box },
  { id: "settings", label: "设置", icon: Settings },
];

const wizardNav: { id: WizardStep; label: string; icon: LucideIcon }[] = [
  { id: "welcome", label: "欢迎", icon: Home },
  { id: "check", label: "环境检测", icon: Search },
  { id: "configure", label: "配置", icon: Settings },
  { id: "install", label: "安装", icon: Download },
];

export function AppSidebar({
  mode, activeTab, onTabChange, wizardStep, onWizardNavigate,
  completedSteps, appVersion,
}: AppSidebarProps) {
  const currentWizardIdx = wizardNav.findIndex((s) => s.id === wizardStep);

  return (
    <Sidebar variant="sidebar">
      <div className="h-6 shrink-0 border-b border-white/[0.05] bg-sidebar/90" data-tauri-drag-region />
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" className="cursor-default hover:bg-transparent">
              <img src="/icon.png" alt="OpenClaw" className="size-7 rounded-lg" />
              <div className="grid flex-1 text-left text-[13px] leading-tight">
                <span className="truncate font-semibold">OpenClaw</span>
                <span className="truncate text-[11px] text-muted-foreground">
                  {mode === "wizard" ? "安装向导" : "控制面板"}
                </span>
              </div>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        {mode === "dashboard" && (
          <SidebarGroup>
            <SidebarGroupLabel>导航</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                {dashboardNav.map((item) => (
                  <SidebarMenuItem key={item.id}>
                    <SidebarMenuButton
                      isActive={activeTab === item.id}
                      onClick={() => onTabChange?.(item.id)}
                      tooltip={item.label}
                    >
                      <item.icon />
                      <span>{item.label}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        )}

        {mode === "wizard" && (
          <SidebarGroup>
            <SidebarGroupLabel>安装步骤</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                {wizardNav.map((item, idx) => {
                  const isActive = item.id === wizardStep;
                  const isCompleted = completedSteps?.has(item.id);
                  const isPast = idx < currentWizardIdx;
                  return (
                    <SidebarMenuItem key={item.id}>
                      <SidebarMenuButton
                        isActive={isActive}
                        onClick={() => onWizardNavigate?.(item.id)}
                        className={!isActive && !isCompleted && !isPast ? "opacity-50" : ""}
                        tooltip={item.label}
                      >
                        {isCompleted && !isActive ? <CheckCircle2 className="text-emerald-500" /> : <item.icon />}
                        <span>{item.label}</span>
                      </SidebarMenuButton>
                      {isCompleted && !isActive && (
                        <SidebarMenuBadge>
                          <div className="w-2 h-2 rounded-full bg-emerald-500" />
                        </SidebarMenuBadge>
                      )}
                    </SidebarMenuItem>
                  );
                })}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        )}
      </SidebarContent>

      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <div className="px-2 py-1.5 text-xs text-muted-foreground">
              OpenClaw Installer · v{appVersion ?? "未知"}
            </div>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  );
}
