import { useState, useEffect, useCallback, useMemo, useRef, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Bot, Trash2, Loader2, RefreshCw, UserPlus, ChevronDown, ChevronUp,
  CheckCircle2, CircleAlert, FolderTree, Route, Check, Save, FileText,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { ConfirmActionDialog } from "@/components/ConfirmActionDialog";
import PageShell from "@/components/PageShell";
import type { AgentInfo, CommandResult, ProviderInfo } from "@/types";

const inputCls = "w-full h-9 px-3 text-[13px] rounded-lg border border-white/[0.08] bg-white/[0.03] text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-primary/50 focus:border-primary/30 transition-colors";
const textareaCls = `${inputCls} min-h-[90px] h-auto py-2 resize-y`;
const editorCls = "min-h-[360px] w-full rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-3 font-mono text-[12px] leading-6 text-foreground placeholder:text-muted-foreground/40 focus:outline-none focus:ring-1 focus:ring-primary/50 focus:border-primary/30 transition-colors resize-y";

interface ModelOption {
  value: string;
  label: string;
}

interface AgentWorkspaceFile {
  name: string;
  path: string;
  exists: boolean;
}

interface AgentWorkspaceSnapshot {
  agentId: string;
  workspaceDir: string;
  selectedFileName: string;
  selectedFileContent: string;
  files: AgentWorkspaceFile[];
}

const WORKSPACE_FILE_DEFS = [
  { name: "AGENTS.md", title: "协作规则", description: "定义这个 Agent 的职责边界、子代理策略和协作方式。" },
  { name: "SOUL.md", title: "人格与风格", description: "定义语气、偏好、目标和这个 Agent 的长期气质。" },
  { name: "TOOLS.md", title: "工具约束", description: "约束它如何使用工具、哪些工具优先、哪些工具禁用。" },
  { name: "IDENTITY.md", title: "身份设定", description: "补充身份信息、背景和角色定位。" },
  { name: "USER.md", title: "用户画像", description: "沉淀这个 Agent 主要服务对象的偏好与背景。" },
  { name: "HEARTBEAT.md", title: "运行节奏", description: "记录周期性检查、状态同步和自检要求。" },
  { name: "BOOTSTRAP.md", title: "启动清单", description: "定义每次启动时应该先做什么检查或准备。" },
] as const;

export default function AgentsPage() {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [primaryModel, setPrimaryModel] = useState("");
  const [modelOptions, setModelOptions] = useState<ModelOption[]>([]);
  const [loading, setLoading] = useState(true);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [pendingDeleteAgent, setPendingDeleteAgent] = useState<AgentInfo | null>(null);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [createError, setCreateError] = useState("");
  const [createSuccess, setCreateSuccess] = useState("");
  const [agentId, setAgentId] = useState("");
  const [agentModel, setAgentModel] = useState("");
  const [workspacePath, setWorkspacePath] = useState("");
  const [agentDirPath, setAgentDirPath] = useState("");
  const [bindingsText, setBindingsText] = useState("");

  const [selectedAgentId, setSelectedAgentId] = useState("");
  const [workspaceSnapshot, setWorkspaceSnapshot] = useState<AgentWorkspaceSnapshot | null>(null);
  const [selectedWorkspaceFile, setSelectedWorkspaceFile] = useState<string>(WORKSPACE_FILE_DEFS[0].name);
  const [workspaceDraft, setWorkspaceDraft] = useState("");
  const [workspaceDirty, setWorkspaceDirty] = useState(false);
  const [workspaceLoading, setWorkspaceLoading] = useState(false);
  const [workspaceSaving, setWorkspaceSaving] = useState(false);
  const [workspaceError, setWorkspaceError] = useState("");
  const [workspaceSuccess, setWorkspaceSuccess] = useState("");

  const modelMenuRef = useRef<HTMLDivElement>(null);
  const createInFlightRef = useRef(false);
  const workspaceCacheRef = useRef<Record<string, AgentWorkspaceSnapshot>>({});
  const workspaceRequestRef = useRef(0);

  const fetchAgents = useCallback(async () => {
    setLoading(true);
    try {
      const [list, primary] = await Promise.all([
        invoke("list_agents") as Promise<AgentInfo[]>,
        invoke("get_primary_model") as Promise<string>,
      ]);
      setAgents(list);
      setPrimaryModel(primary);
      setSelectedAgentId((current) => {
        if (current && list.some((agent) => agent.id === current)) {
          return current;
        }
        return list[0]?.id ?? "";
      });
    } catch {
      setAgents([]);
      setPrimaryModel("");
      setSelectedAgentId("");
    } finally {
      setLoading(false);
    }
  }, []);

  const loadModelOptions = useCallback(async (primaryOverride?: string) => {
    try {
      const providers = await (invoke("list_providers") as Promise<ProviderInfo[]>);
      setModelOptions(buildModelOptions(providers, primaryOverride ?? primaryModel));
    } catch {
      setModelOptions([]);
    }
  }, [primaryModel]);

  const applyWorkspaceSnapshot = useCallback((snapshot: AgentWorkspaceSnapshot) => {
    setWorkspaceSnapshot(snapshot);
    setSelectedWorkspaceFile(snapshot.selectedFileName || WORKSPACE_FILE_DEFS[0].name);
    setWorkspaceSuccess("");
  }, []);

  const loadWorkspaceSnapshot = useCallback(async (
    agentToLoad: AgentInfo | null,
    preferredFile?: string,
    options?: { force?: boolean },
  ) => {
    if (!agentToLoad) {
      setWorkspaceSnapshot(null);
      setWorkspaceLoading(false);
      return;
    }

    const requestedFile = preferredFile ?? WORKSPACE_FILE_DEFS[0].name;
    const cachedSnapshot = workspaceCacheRef.current[agentToLoad.id];
    if (!options?.force && cachedSnapshot && cachedSnapshot.selectedFileName === requestedFile) {
      setWorkspaceLoading(false);
      setWorkspaceError("");
      applyWorkspaceSnapshot(cachedSnapshot);
      return;
    }

    const requestId = ++workspaceRequestRef.current;
    setWorkspaceLoading(true);
    setWorkspaceError("");
    try {
      const result: CommandResult = await invoke("get_agent_workspace_snapshot", {
        id: agentToLoad.id,
        workspaceDir: agentToLoad.workspace || null,
        fileName: requestedFile,
      });
      if (requestId !== workspaceRequestRef.current) {
        return;
      }
      if (!result.success) {
        setWorkspaceSnapshot(null);
        setWorkspaceError(result.stderr || "工作区文件加载失败");
        return;
      }

      const snapshot = parseWorkspaceSnapshot(result.stdout);
      if (!snapshot) {
        setWorkspaceSnapshot(null);
        setWorkspaceError("工作区文件解析失败");
        return;
      }

      workspaceCacheRef.current[agentToLoad.id] = snapshot;
      applyWorkspaceSnapshot(snapshot);
    } catch (error) {
      if (requestId !== workspaceRequestRef.current) {
        return;
      }
      setWorkspaceSnapshot(null);
      setWorkspaceError(`${error}`);
    } finally {
      if (requestId === workspaceRequestRef.current) {
        setWorkspaceLoading(false);
      }
    }
  }, [applyWorkspaceSnapshot]);

  const activeAgent = useMemo(
    () => agents.find((agent) => agent.id === selectedAgentId) ?? agents[0] ?? null,
    [agents, selectedAgentId],
  );

  useEffect(() => { void fetchAgents(); }, [fetchAgents]);

  useEffect(() => {
    workspaceRequestRef.current += 1;

    if (!activeAgent) {
      setWorkspaceSnapshot(null);
      setWorkspaceDraft("");
      setWorkspaceDirty(false);
      setWorkspaceLoading(false);
      setWorkspaceError("");
      setWorkspaceSuccess("");
      return;
    }

    const cachedSnapshot = workspaceCacheRef.current[activeAgent.id];
    if (cachedSnapshot) {
      setWorkspaceLoading(false);
      setWorkspaceError("");
      applyWorkspaceSnapshot(cachedSnapshot);
      return;
    }

    void loadWorkspaceSnapshot(activeAgent, WORKSPACE_FILE_DEFS[0].name, { force: true });
  }, [activeAgent, applyWorkspaceSnapshot, loadWorkspaceSnapshot]);

  useEffect(() => {
    if (!createDialogOpen) {
      return;
    }
    void loadModelOptions();
  }, [createDialogOpen, loadModelOptions]);

  useEffect(() => {
    if (!modelMenuOpen) {
      return undefined;
    }

    const handlePointerDown = (event: MouseEvent) => {
      if (modelMenuRef.current && !modelMenuRef.current.contains(event.target as Node)) {
        setModelMenuOpen(false);
      }
    };

    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [modelMenuOpen]);

  const activeWorkspaceFile = useMemo(
    () => workspaceSnapshot?.files.find((file) => file.name === selectedWorkspaceFile) ?? null,
    [workspaceSnapshot, selectedWorkspaceFile],
  );

  useEffect(() => {
    setWorkspaceDraft(workspaceSnapshot?.selectedFileContent ?? "");
    setWorkspaceDirty(false);
    setWorkspaceError("");
    setWorkspaceSuccess("");
  }, [workspaceSnapshot?.agentId, workspaceSnapshot?.selectedFileName, workspaceSnapshot?.selectedFileContent]);

  const selectedModel = modelOptions.find((option) => option.value === agentModel) ?? null;
  const selectedModelLabel = selectedModel?.value || primaryModel || "沿用当前默认模型";
  const suggestedRoot = agentId.trim() ? `~/.openclaw/agents/${agentId.trim()}` : "~/.openclaw/agents/<agent-id>";
  const suggestedWorkspace = workspacePath.trim() || `${suggestedRoot}/workspace`;
  const suggestedAgentDir = agentDirPath.trim() || `${suggestedRoot}/agent`;
  const parsedBindings = parseBindings(bindingsText);
  const activeWorkspaceFileMeta = WORKSPACE_FILE_DEFS.find((file) => file.name === selectedWorkspaceFile) ?? WORKSPACE_FILE_DEFS[0];

  const confirmDiscardDraft = () => {
    if (!workspaceDirty) return true;
    return confirm("当前文件有未保存修改，确定放弃这些变更吗？");
  };

  const handleSelectAgent = (agentIdToSelect: string) => {
    if (agentIdToSelect === selectedAgentId) return;
    if (!confirmDiscardDraft()) return;
    setSelectedAgentId(agentIdToSelect);
  };

  const handleSelectWorkspaceFile = (fileName: string) => {
    if (fileName === selectedWorkspaceFile) return;
    if (!confirmDiscardDraft()) return;
    setSelectedWorkspaceFile(fileName);
    if (activeAgent) {
      void loadWorkspaceSnapshot(activeAgent, fileName, { force: true });
    }
  };

  const handleDelete = (agent: AgentInfo, event?: ReactMouseEvent) => {
    event?.preventDefault();
    event?.stopPropagation();
    setPendingDeleteAgent(agent);
  };

  const resetCreateForm = () => {
    setAgentId("");
    setAgentModel("");
    setWorkspacePath("");
    setAgentDirPath("");
    setBindingsText("");
    setCreateError("");
    setCreateSuccess("");
    setAdvancedOpen(false);
    setModelMenuOpen(false);
  };

  const openCreateDialog = () => {
    setCreateError("");
    setModelMenuOpen(false);
    setCreateDialogOpen(true);
  };

  const closeCreateDialog = () => {
    if (creating) return;
    setModelMenuOpen(false);
    setCreateDialogOpen(false);
  };

  const handleConfirmDelete = async () => {
    if (!pendingDeleteAgent) return;

    setDeleting(pendingDeleteAgent.id);
    try {
      const result: CommandResult = await invoke("delete_agent", { id: pendingDeleteAgent.id });
      if (result.success) {
        delete workspaceCacheRef.current[pendingDeleteAgent.id];
        setAgents((prev) => prev.filter((agent) => agent.id !== pendingDeleteAgent.id));
        setSelectedAgentId((current) => (current === pendingDeleteAgent.id ? "" : current));
        setPendingDeleteAgent(null);
      } else {
        alert(result.stderr || "删除 Agent 失败");
      }
    } catch (error) {
      alert(`删除 Agent 失败: ${error}`);
    } finally {
      setDeleting(null);
    }
  };

  const handleCreate = async () => {
    if (createInFlightRef.current) return;

    const trimmedId = agentId.trim();
    if (!trimmedId) {
      setCreateError("请先填写 Agent ID");
      setCreateSuccess("");
      return;
    }

    createInFlightRef.current = true;
    setCreating(true);
    setCreateError("");
    setCreateSuccess("");

    try {
      const result: CommandResult = await invoke("create_agent", {
        id: trimmedId,
        model: agentModel.trim() || null,
        workspace: workspacePath.trim() || null,
        agentDir: agentDirPath.trim() || null,
        bindings: parseBindings(bindingsText),
      });

      if (!result.success) {
        setCreateError(result.stderr || "创建 Agent 失败");
        return;
      }

      setCreateSuccess(result.stdout || `已创建 Agent "${trimmedId}"`);
      setCreateDialogOpen(false);
      setAgentId("");
      setAgentModel("");
      setWorkspacePath("");
      setAgentDirPath("");
      setBindingsText("");
      setModelMenuOpen(false);
      setAdvancedOpen(false);
      delete workspaceCacheRef.current[trimmedId.toLowerCase()];
      setSelectedAgentId(trimmedId.toLowerCase());
      await fetchAgents();
    } catch (error) {
      setCreateError(`创建 Agent 失败: ${error}`);
    } finally {
      createInFlightRef.current = false;
      setCreating(false);
    }
  };

  const handleSaveWorkspaceFile = async () => {
    if (!activeAgent || !activeWorkspaceFile) return;
    setWorkspaceSaving(true);
    setWorkspaceError("");
    setWorkspaceSuccess("");

    try {
      const result: CommandResult = await invoke("save_agent_workspace_file", {
        id: activeAgent.id,
        workspaceDir: activeAgent.workspace || null,
        fileName: activeWorkspaceFile.name,
        content: workspaceDraft,
      });

      if (!result.success) {
        setWorkspaceError(result.stderr || "保存失败");
        return;
      }

      setWorkspaceSuccess(result.stdout || `已保存 ${activeWorkspaceFile.name}`);
      await loadWorkspaceSnapshot(activeAgent, activeWorkspaceFile.name, { force: true });
    } catch (error) {
      setWorkspaceError(`保存失败: ${error}`);
    } finally {
      setWorkspaceSaving(false);
    }
  };

  const handleReloadWorkspaceFile = async () => {
    if (!activeAgent) return;
    if (!confirmDiscardDraft()) return;
    await loadWorkspaceSnapshot(activeAgent, selectedWorkspaceFile, { force: true });
  };

  return (
    <TooltipProvider delayDuration={300}>
      <PageShell
        header={(
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2.5">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-sky-500/10">
                <Bot size={15} className="text-sky-400" />
              </div>
              <div>
                <h2 className="text-sm font-semibold">Agents 管理</h2>
                <p className="text-[11px] text-muted-foreground">{loading ? "加载中" : `${agents.length} 个 Agent`}</p>
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" size="icon" className="h-7 w-7" onClick={() => void fetchAgents()} disabled={loading || creating || workspaceSaving}>
                    {loading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>刷新</TooltipContent>
              </Tooltip>
              <Button size="sm" onClick={openCreateDialog}>
                <UserPlus />
                新建 Agent
              </Button>
            </div>
          </div>
        )}
      >

          {createSuccess && (
            <div className="flex items-start gap-2 rounded-lg border border-emerald-500/20 bg-emerald-500/8 px-3 py-2.5 text-[12px] text-emerald-300">
              <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-emerald-400" />
              <span className="flex-1">{createSuccess}</span>
              <Button size="sm" variant="ghost" className="h-6 px-2 text-[11px] text-emerald-200 hover:text-white" onClick={() => setCreateSuccess("")}>
                关闭
              </Button>
            </div>
          )}

          <Card className="border-sky-500/15 bg-sky-500/[0.04]">
            <CardContent className="flex flex-col gap-3 p-4 lg:flex-row lg:items-center lg:justify-between">
              <div className="space-y-1">
                <h3 className="text-[13px] font-semibold">独立 Agent 工作区</h3>
                <p className="text-[11px] text-muted-foreground">
                  新 Agent 会在弹窗里创建，默认分配独立 workspace、agentDir 和基础工作区文件。
                </p>
              </div>
              <div className="rounded-lg border border-white/[0.06] bg-white/[0.03] px-3 py-2 text-[11px] text-muted-foreground">
                当前主模型：
                <span className="ml-1 font-mono text-foreground/80">{primaryModel || "未设置，创建时将沿用默认配置"}</span>
              </div>
            </CardContent>
          </Card>

          {loading ? (
            <div className="flex items-center justify-center py-20 text-muted-foreground">
              <Loader2 size={18} className="mr-2 animate-spin" />
              <span className="text-[13px]">加载中...</span>
            </div>
          ) : agents.length === 0 ? (
            <EmptyState onCreate={openCreateDialog} />
          ) : (
            <>
              <div className="grid gap-4 xl:grid-cols-[320px_minmax(0,1fr)]">
                <Card className="border-white/[0.08] xl:sticky xl:top-5 xl:self-start">
                  <CardContent className="p-0">
                    <div className="border-b border-white/[0.06] px-4 py-3">
                      <div className="flex items-center justify-between gap-3">
                        <div>
                          <h3 className="text-[13px] font-semibold">Agent 列表</h3>
                          <p className="text-[11px] text-muted-foreground">
                            选中后右侧立即进入该 Agent 的工作区文件编辑。
                          </p>
                        </div>
                        <Badge className="h-5 border-0 bg-white/[0.06] px-2 text-[10px] text-foreground/70">
                          {agents.length} 个
                        </Badge>
                      </div>
                    </div>
                    <div className="max-h-[320px] overflow-y-auto p-3 xl:h-[calc(100vh-250px)] xl:max-h-[calc(100vh-250px)]">
                      <div className="space-y-2 pr-1">
                        {agents.map((agent) => (
                          <AgentListItem
                            key={agent.id}
                            agent={agent}
                            selected={agent.id === activeAgent?.id}
                            deleting={deleting === agent.id}
                            onSelect={() => handleSelectAgent(agent.id)}
                            onDelete={() => handleDelete(agent)}
                          />
                        ))}
                      </div>
                    </div>
                  </CardContent>
                </Card>

                {activeAgent ? (
                <Card className="border-sky-500/15">
                  <CardContent className="p-5 space-y-4">
                    <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                      <div className="space-y-1">
                        <div className="flex items-center gap-2">
                          <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-sky-500/10">
                            <FileText size={15} className="text-sky-400" />
                          </div>
                          <div>
                            <h3 className="text-[13px] font-semibold">Agent 工作区文件</h3>
                            <p className="text-[11px] text-muted-foreground">
                              为当前 Agent 维护人格、规则和长期配置文件，直接作用到它的独立工作区。
                            </p>
                          </div>
                        </div>
                      </div>
                      <div className="rounded-lg border border-white/[0.06] bg-white/[0.03] px-3 py-2 text-[11px] text-muted-foreground">
                        编辑对象：
                        <span className="ml-1 font-mono text-foreground/80">{activeAgent.id}</span>
                        {workspaceSnapshot?.workspaceDir && (
                          <span className="mt-1 block break-all font-mono text-[10px] text-foreground/60">
                            {workspaceSnapshot.workspaceDir}
                          </span>
                        )}
                      </div>
                    </div>

                    {workspaceError && (
                      <div className="flex items-start gap-2 rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                        <CircleAlert size={14} className="mt-0.5 shrink-0 text-red-400" />
                        <span>{workspaceError}</span>
                      </div>
                    )}

                    {workspaceSuccess && (
                      <div className="flex items-start gap-2 rounded-lg border border-emerald-500/20 bg-emerald-500/8 px-3 py-2.5 text-[12px] text-emerald-300">
                        <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-emerald-400" />
                        <span>{workspaceSuccess}</span>
                      </div>
                    )}

                    {workspaceLoading ? (
                      <div className="flex items-center justify-center py-16 text-muted-foreground">
                        <Loader2 size={18} className="mr-2 animate-spin" />
                        <span className="text-[13px]">正在读取工作区文件...</span>
                      </div>
                    ) : (
                      <div className="grid gap-4 lg:grid-cols-[220px_minmax(0,1fr)]">
                        <div className="space-y-2">
                          {WORKSPACE_FILE_DEFS.map((file) => {
                            const snapshotFile = workspaceSnapshot?.files.find((item) => item.name === file.name);
                            const active = file.name === selectedWorkspaceFile;
                            return (
                              <button
                                key={file.name}
                                type="button"
                                onClick={() => handleSelectWorkspaceFile(file.name)}
                                className={`w-full rounded-xl border px-3 py-3 text-left transition-colors ${
                                  active
                                    ? "border-sky-500/25 bg-sky-500/10"
                                    : "border-white/[0.08] bg-white/[0.02] hover:border-white/[0.16] hover:bg-white/[0.04]"
                                }`}
                              >
                                <div className="flex items-center justify-between gap-2">
                                  <span className="font-mono text-[12px] text-foreground/80">{file.name}</span>
                                  <Badge className={`h-4 border-0 px-1.5 text-[10px] ${
                                    snapshotFile?.exists
                                      ? "bg-emerald-500/10 text-emerald-300"
                                      : "bg-white/[0.06] text-muted-foreground"
                                  }`}>
                                    {snapshotFile?.exists ? "已存在" : "未创建"}
                                  </Badge>
                                </div>
                                <p className="mt-1 text-[11px] leading-5 text-muted-foreground">
                                  {file.title}
                                </p>
                              </button>
                            );
                          })}
                        </div>

                        <div className="space-y-3">
                          <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3">
                            <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                              <div className="space-y-1">
                                <div className="flex items-center gap-2">
                                  <span className="font-mono text-[13px] text-foreground/85">{activeWorkspaceFileMeta.name}</span>
                                  <Badge className={`h-4 border-0 px-1.5 text-[10px] ${
                                    activeWorkspaceFile?.exists
                                      ? "bg-emerald-500/10 text-emerald-300"
                                      : "bg-amber-500/10 text-amber-300"
                                  }`}>
                                    {activeWorkspaceFile?.exists ? "文件已存在" : "保存后创建"}
                                  </Badge>
                                </div>
                                <p className="text-[11px] leading-5 text-muted-foreground">
                                  {activeWorkspaceFileMeta.description}
                                </p>
                              </div>
                              <div className="rounded-lg border border-white/[0.06] bg-black/10 px-2.5 py-2 text-[10px] text-muted-foreground">
                                <div className="uppercase tracking-widest">Path</div>
                                <div className="mt-1 max-w-[420px] break-all font-mono text-foreground/65">
                                  {activeWorkspaceFile?.path ?? "—"}
                                </div>
                              </div>
                            </div>
                          </div>

                          <textarea
                            className={editorCls}
                            value={workspaceDraft}
                            onChange={(event) => {
                              setWorkspaceDraft(event.target.value);
                              setWorkspaceDirty(true);
                            }}
                            placeholder={`在这里编辑 ${activeWorkspaceFileMeta.name} ...`}
                          />

                          <div className="flex flex-wrap items-center gap-2">
                            <Button size="sm" onClick={() => void handleSaveWorkspaceFile()} disabled={workspaceSaving || !activeAgent}>
                              {workspaceSaving ? <Loader2 className="animate-spin" /> : <Save />}
                              {workspaceSaving ? "保存中..." : "保存当前文件"}
                            </Button>
                            <Button
                              size="sm"
                              variant="outline"
                              onClick={() => {
                                setWorkspaceDraft(workspaceSnapshot?.selectedFileContent ?? "");
                                setWorkspaceDirty(false);
                                setWorkspaceError("");
                                setWorkspaceSuccess("");
                              }}
                              disabled={workspaceSaving}
                            >
                              恢复已加载内容
                            </Button>
                            <Button size="sm" variant="outline" onClick={() => void handleReloadWorkspaceFile()} disabled={workspaceSaving || workspaceLoading}>
                              <RefreshCw size={14} />
                              重新读取
                            </Button>
                            {workspaceDirty && (
                              <span className="text-[11px] text-amber-300">当前文件有未保存修改</span>
                            )}
                          </div>
                        </div>
                      </div>
                    )}
                  </CardContent>
                </Card>
                ) : (
                  <Card className="border-white/[0.08]">
                    <CardContent className="flex min-h-[320px] flex-col items-center justify-center gap-3 p-6 text-center">
                      <div className="flex h-12 w-12 items-center justify-center rounded-2xl bg-white/[0.04]">
                        <Bot size={20} className="text-muted-foreground" />
                      </div>
                      <div>
                        <h3 className="text-sm font-medium">先选一个 Agent</h3>
                        <p className="mt-1 text-[12px] text-muted-foreground">
                          左侧列表里选中 Agent 后，这里会直接显示它的工作区文件编辑器。
                        </p>
                      </div>
                    </CardContent>
                  </Card>
                )}
              </div>
            </>
          )}
      </PageShell>
      {createDialogOpen && (
        <div
          className="fixed inset-0 z-[120] flex items-center justify-center bg-black/70 px-4 py-6 backdrop-blur-sm"
          onClick={closeCreateDialog}
        >
          <Card
            className="w-full max-w-3xl border-white/[0.08] bg-[#081017] shadow-2xl shadow-black/40"
            onClick={(event) => event.stopPropagation()}
          >
            <CardContent className="max-h-[85vh] overflow-auto p-5 space-y-4">
              <div className="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
                <div className="space-y-1">
                  <div className="flex items-center gap-2">
                    <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-sky-500/10">
                      <UserPlus size={15} className="text-sky-400" />
                    </div>
                    <div>
                      <h3 className="text-[13px] font-semibold">手动创建 Agent</h3>
                      <p className="text-[11px] text-muted-foreground">
                        按官方 `openclaw agents add` 的方式创建，每个 Agent 默认使用独立的 workspace 和 agentDir。
                      </p>
                    </div>
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <div className="rounded-lg border border-white/[0.06] bg-white/[0.03] px-3 py-2 text-[11px] text-muted-foreground">
                    当前主模型：
                    <span className="ml-1 font-mono text-foreground/80">{primaryModel || "未设置，创建时将沿用默认配置"}</span>
                  </div>
                  <Button size="sm" variant="ghost" onClick={closeCreateDialog} disabled={creating}>
                    关闭
                  </Button>
                </div>
              </div>

              <div className="grid gap-3 md:grid-cols-2">
                <div>
                  <label className="mb-1.5 block text-[12px] text-muted-foreground">Agent ID</label>
                  <input
                    className={inputCls}
                    placeholder="例如 sales-bot"
                    value={agentId}
                    onChange={(event) => setAgentId(event.target.value)}
                  />
                  <p className="mt-1 text-[11px] text-muted-foreground">
                    建议使用英文、数字、`-`、`_`，每个 ID 都会拥有自己的独立目录。
                  </p>
                </div>
                <div>
                  <label className="mb-1.5 block text-[12px] text-muted-foreground">绑定模型</label>
                  <div className="relative" ref={modelMenuRef}>
                    <button
                      type="button"
                      className="flex w-full items-center justify-between gap-3 rounded-xl border border-white/[0.08] bg-white/[0.03] px-3 py-2.5 text-left transition-colors hover:border-sky-500/25 hover:bg-white/[0.05] focus:outline-none focus:ring-1 focus:ring-primary/50"
                      onClick={() => setModelMenuOpen((value) => !value)}
                      disabled={modelOptions.length === 0}
                    >
                      <div className="min-w-0">
                        <p className="text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                          {agentModel ? "已选模型" : "默认模型"}
                        </p>
                        <div className="mt-1 flex items-center gap-2">
                          <span className="truncate font-mono text-[12px] text-foreground/85">{selectedModelLabel}</span>
                          {!agentModel && (
                            <Badge className="h-4 border-0 bg-sky-500/10 px-1.5 text-[10px] text-sky-400">
                              默认
                            </Badge>
                          )}
                        </div>
                      </div>
                      <ChevronDown
                        size={15}
                        className={`shrink-0 text-muted-foreground transition-transform ${modelMenuOpen ? "rotate-180" : ""}`}
                      />
                    </button>

                    {modelMenuOpen && modelOptions.length > 0 && (
                      <div className="absolute left-0 right-0 top-[calc(100%+8px)] z-20 overflow-hidden rounded-xl border border-white/[0.08] bg-[#10141b] shadow-2xl shadow-black/35">
                        <div className="border-b border-white/[0.06] px-3 py-2 text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                          选择 Agent 模型
                        </div>
                        <ScrollArea className="max-h-60">
                          <div className="space-y-1 p-2">
                            <ModelMenuItem
                              label={primaryModel ? `沿用当前默认模型 (${primaryModel})` : "沿用当前默认模型"}
                              selected={!agentModel}
                              onSelect={() => {
                                setAgentModel("");
                                setModelMenuOpen(false);
                              }}
                            />
                            {modelOptions.map((option) => (
                              <ModelMenuItem
                                key={option.value}
                                label={option.label}
                                selected={option.value === agentModel}
                                onSelect={() => {
                                  setAgentModel(option.value);
                                  setModelMenuOpen(false);
                                }}
                              />
                            ))}
                          </div>
                        </ScrollArea>
                      </div>
                    )}
                  </div>
                  <p className="mt-1 text-[11px] text-muted-foreground">
                    {modelOptions.length > 0
                      ? "从已同步的模型里直接选择；留空时按 OpenClaw 默认模型处理。"
                      : "当前还没有可选模型，请先去模型页同步 Provider 和模型。"}
                  </p>
                </div>
              </div>

              <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-[11px] text-muted-foreground">
                <div className="mb-2 flex items-center gap-2 text-foreground/80">
                  <FolderTree size={13} className="text-sky-400" />
                  <span className="font-medium">独立目录预览</span>
                </div>
                <div className="space-y-1.5">
                  <InfoRow label="Workspace" value={suggestedWorkspace} />
                  <InfoRow label="Agent Dir" value={suggestedAgentDir} />
                  <InfoRow
                    label="Bindings"
                    value={parsedBindings.length > 0 ? parsedBindings.join(", ") : "未配置，后续可在 OpenClaw 中继续补充"}
                  />
                </div>
              </div>

              <Button
                size="sm"
                variant="ghost"
                className="h-8 px-2 text-[12px]"
                onClick={() => setAdvancedOpen((value) => !value)}
              >
                {advancedOpen ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                {advancedOpen ? "收起高级选项" : "展开高级选项"}
              </Button>

              {advancedOpen && (
                <div className="grid gap-3 md:grid-cols-2">
                  <div>
                    <label className="mb-1.5 block text-[12px] text-muted-foreground">自定义 Workspace</label>
                    <input
                      className={inputCls}
                      placeholder={suggestedWorkspace}
                      value={workspacePath}
                      onChange={(event) => setWorkspacePath(event.target.value)}
                    />
                  </div>
                  <div>
                    <label className="mb-1.5 block text-[12px] text-muted-foreground">自定义 Agent Dir</label>
                    <input
                      className={inputCls}
                      placeholder={suggestedAgentDir}
                      value={agentDirPath}
                      onChange={(event) => setAgentDirPath(event.target.value)}
                    />
                  </div>
                  <div className="md:col-span-2">
                    <label className="mb-1.5 block text-[12px] text-muted-foreground">路由绑定</label>
                    <textarea
                      className={textareaCls}
                      placeholder={"一行一个，或用逗号分隔\n例如 slack:sales\nwechat:vip"}
                      value={bindingsText}
                      onChange={(event) => setBindingsText(event.target.value)}
                    />
                    <p className="mt-1 text-[11px] text-muted-foreground">
                      用于把特定入口或账号路由到这个 Agent；留空也可以，后续再补。
                    </p>
                  </div>
                </div>
              )}

              {createError && (
                <div className="flex items-start gap-2 rounded-lg border border-red-500/20 bg-red-500/8 px-3 py-2.5 text-[12px] text-red-300">
                  <CircleAlert size={14} className="mt-0.5 shrink-0 text-red-400" />
                  <span>{createError}</span>
                </div>
              )}

              {creating && (
                <div className="flex items-start gap-2 rounded-lg border border-sky-500/20 bg-sky-500/8 px-3 py-2.5 text-[12px] text-sky-200">
                  <Loader2 size={14} className="mt-0.5 shrink-0 animate-spin text-sky-300" />
                  <span>正在调用 `openclaw agents add` 创建独立 Agent，这一步通常会持续几秒，请稍等。</span>
                </div>
              )}

              <div className="flex flex-wrap gap-2">
                <Button size="sm" onClick={() => void handleCreate()} disabled={creating || !agentId.trim()}>
                  {creating ? <Loader2 className="animate-spin" /> : <UserPlus />}
                  {creating ? "创建中..." : "创建 Agent"}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={resetCreateForm}
                  disabled={creating}
                >
                  清空表单
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>
      )}
      <ConfirmActionDialog
        open={Boolean(pendingDeleteAgent)}
        title={pendingDeleteAgent ? `删除 Agent “${pendingDeleteAgent.name}”？` : "删除 Agent"}
        description="这个 Agent 的配置和目录会被移除。确认后才会真正执行。"
        confirmLabel="确认删除"
        destructive
        busy={Boolean(deleting)}
        onCancel={() => {
          if (!deleting) {
            setPendingDeleteAgent(null);
          }
        }}
        onConfirm={() => void handleConfirmDelete()}
      />
    </TooltipProvider>
  );
}

function AgentListItem({
  agent,
  selected,
  deleting,
  onSelect,
  onDelete,
}: {
  agent: AgentInfo;
  selected: boolean;
  deleting: boolean;
  onSelect: () => void;
  onDelete: () => void;
}) {
  return (
    <button
      type="button"
      className={`group w-full rounded-xl border px-3 py-3 text-left transition-colors ${
        selected
          ? "border-sky-500/30 bg-sky-500/[0.06]"
          : "border-white/[0.08] bg-white/[0.02] hover:border-sky-500/20 hover:bg-white/[0.04]"
      }`}
      onClick={onSelect}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-sky-500/10">
              <Bot size={13} className="text-sky-400" />
            </div>
            <div className="min-w-0">
              <p className="truncate text-[13px] font-medium">{agent.name}</p>
              <p className="truncate text-[10px] font-mono text-muted-foreground">{agent.id}</p>
            </div>
          </div>
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <Badge className="h-4 border-0 bg-white/[0.06] px-1.5 text-[10px] text-foreground/70">
              {agent.model || "沿用默认模型"}
            </Badge>
            {selected && (
              <Badge className="h-4 border-0 bg-sky-500/10 px-1.5 text-[10px] text-sky-300">
                正在编辑
              </Badge>
            )}
            {agent.bindings.length > 0 && (
              <Badge className="h-4 border-0 bg-teal-500/10 px-1.5 text-[10px] text-teal-300">
                {agent.bindings.length} 个绑定
              </Badge>
            )}
          </div>
        </div>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-7 w-7 shrink-0 text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100 focus-visible:opacity-100"
              onMouseDown={(event) => {
                event.preventDefault();
                event.stopPropagation();
              }}
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                onDelete();
              }}
              disabled={deleting}
            >
              {deleting ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
            </Button>
          </TooltipTrigger>
          <TooltipContent>删除</TooltipContent>
        </Tooltip>
      </div>

      {agent.description && (
        <p className="mt-2 line-clamp-2 text-[11px] text-muted-foreground">{agent.description}</p>
      )}

      <div className="mt-3 space-y-1.5">
        <InfoRow label="Workspace" value={agent.workspace || "未配置"} />
        <InfoRow label="Agent Dir" value={agent.agentDir || agent.path} />
      </div>

      {agent.bindings.length > 0 && (
        <div className="mt-3">
          <p className="mb-1 flex items-center gap-1.5 text-[10px] text-muted-foreground">
            <Route size={11} />
            路由绑定
          </p>
          <div className="flex flex-wrap gap-1">
            {agent.bindings.slice(0, 3).map((binding) => (
              <Badge key={binding} className="h-4 border-0 bg-sky-500/10 px-1.5 text-[10px] text-sky-400">
                {binding}
              </Badge>
            ))}
            {agent.bindings.length > 3 && (
              <Badge className="h-4 border-0 bg-white/[0.06] px-1.5 text-[10px] text-foreground/70">
                +{agent.bindings.length - 3}
              </Badge>
            )}
          </div>
        </div>
      )}
    </button>
  );
}

function EmptyState({ onCreate }: { onCreate: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center py-16 text-center">
      <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-white/[0.04]">
        <UserPlus size={24} className="text-muted-foreground" />
      </div>
      <h3 className="mb-1 text-sm font-medium">还没有配置 Agent</h3>
      <p className="max-w-[320px] text-[12px] text-muted-foreground">
        通过新建弹窗可以按官方 `agents add` 方式创建 Agent，并默认分配独立工作区和目录。
      </p>
      <Button size="sm" className="mt-4" onClick={onCreate}>
        <UserPlus />
        新建第一个 Agent
      </Button>
    </div>
  );
}

function InfoRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-2 text-[11px]">
      <span className="w-[74px] shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 break-all font-mono text-foreground/75">{value}</span>
    </div>
  );
}

function parseBindings(value: string) {
  return value
    .split(/[\n,]+/)
    .map((binding) => binding.trim())
    .filter(Boolean);
}

function parseWorkspaceSnapshot(raw: string) {
  try {
    return JSON.parse(raw) as AgentWorkspaceSnapshot;
  } catch {
    return null;
  }
}

function ModelMenuItem({
  label,
  selected,
  onSelect,
}: {
  label: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className={`flex w-full items-center justify-between gap-3 rounded-lg px-3 py-2 text-left text-[12px] transition-colors ${
        selected
          ? "bg-sky-500/12 text-sky-300"
          : "text-foreground/80 hover:bg-white/[0.05]"
      }`}
      onClick={onSelect}
    >
      <span className="min-w-0 truncate font-mono">{label}</span>
      <Check size={13} className={selected ? "shrink-0 opacity-100" : "shrink-0 opacity-0"} />
    </button>
  );
}

function buildModelOptions(providers: ProviderInfo[], primaryModel: string) {
  return providers
    .flatMap((provider) => provider.models.map((model) => {
      const value = `${provider.name}/${model.id}`;
      return {
        value,
        label: value === primaryModel ? `${value} (当前主模型)` : value,
      };
    }))
    .sort((left, right) => {
      if (left.value === primaryModel) return -1;
      if (right.value === primaryModel) return 1;
      return left.value.localeCompare(right.value);
    });
}
