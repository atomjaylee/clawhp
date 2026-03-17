import { useState, useEffect, useCallback, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Box, Trash2, Loader2, RefreshCw, Plus, Star, ChevronDown, ChevronUp,
  Globe, Key, Check, X, Cpu,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { ConfirmActionDialog } from "@/components/ConfirmActionDialog";
import type { ProviderInfo, CommandResult } from "@/types";

type View = "list" | "add" | "sync" | "refresh";
type PendingDelete =
  | { kind: "provider"; providerName: string }
  | { kind: "model"; providerName: string; modelId: string; isPrimary: boolean };
type RefreshCandidateStatus = "existing" | "new" | "missing";

const inputCls = "w-full h-8 px-3 text-[13px] rounded-lg border border-white/[0.08] bg-white/[0.03] text-foreground placeholder:text-muted-foreground/50 focus:outline-none focus:ring-1 focus:ring-primary/50 focus:border-primary/30 transition-colors";

interface ProviderPreset {
  id: string;
  title: string;
  badge: string;
  description: string;
  baseUrl: string;
  defaultName: string;
  defaultKey?: string;
}

interface RefreshCandidate {
  id: string;
  status: RefreshCandidateStatus;
}

const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    id: "openai",
    title: "OpenAI",
    badge: "官方",
    description: "GPT 系列直连入口，适合官方 API 工作流。",
    baseUrl: "https://api.openai.com/v1",
    defaultName: "openai",
  },
  {
    id: "openrouter",
    title: "OpenRouter",
    badge: "聚合",
    description: "聚合多家模型，适合快速对比和补齐模型池。",
    baseUrl: "https://openrouter.ai/api/v1",
    defaultName: "openrouter",
  },
  {
    id: "ollama",
    title: "Ollama",
    badge: "本地",
    description: "本地模型与局域网推理，适合离线和隐私优先工作流。",
    baseUrl: "http://127.0.0.1:11434/v1",
    defaultName: "ollama",
    defaultKey: "ollama",
  },
];

export default function ModelsPage() {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [primaryModel, setPrimaryModel] = useState("");
  const [loading, setLoading] = useState(true);
  const [settingPrimaryRef, setSettingPrimaryRef] = useState("");
  const [view, setView] = useState<View>("list");

  const [addUrl, setAddUrl] = useState("");
  const [addKey, setAddKey] = useState("");
  const [addName, setAddName] = useState("");
  const [fetchingModels, setFetchingModels] = useState(false);
  const [remoteModels, setRemoteModels] = useState<string[]>([]);
  const [selectedModels, setSelectedModels] = useState<Set<string>>(new Set());
  const [syncing, setSyncing] = useState(false);
  const [refreshingProviderName, setRefreshingProviderName] = useState("");
  const [refreshApplying, setRefreshApplying] = useState(false);
  const [refreshProvider, setRefreshProvider] = useState<ProviderInfo | null>(null);
  const [refreshCandidates, setRefreshCandidates] = useState<RefreshCandidate[]>([]);
  const [refreshSelectedModels, setRefreshSelectedModels] = useState<Set<string>>(new Set());
  const [pendingDelete, setPendingDelete] = useState<PendingDelete | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);

  const applyPreset = (preset: ProviderPreset) => {
    setAddUrl(preset.baseUrl);
    setAddName(preset.defaultName);
    setAddKey(preset.defaultKey ?? "");
    setView("add");
  };

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      const [provs, primary] = await Promise.all([
        invoke("list_providers") as Promise<ProviderInfo[]>,
        invoke("get_primary_model") as Promise<string>,
      ]);
      setProviders(provs);
      setPrimaryModel(primary);
    } catch { /* ignore */ }
    finally { setLoading(false); }
  }, []);

  useEffect(() => { fetchData(); }, [fetchData]);

  const handleDeleteProvider = (name: string, event?: MouseEvent) => {
    event?.preventDefault();
    event?.stopPropagation();
    setPendingDelete({ kind: "provider", providerName: name });
  };

  const handleSetPrimary = async (ref: string) => {
    setSettingPrimaryRef(ref);
    try {
      const r: CommandResult = await invoke("set_primary_model", { modelRef: ref });
      if (r.success) {
        await fetchData();
      } else {
        alert(r.stderr || "设置主模型失败");
      }
    } catch (e) {
      alert(`错误: ${e}`);
    } finally {
      setSettingPrimaryRef("");
    }
  };

  const handleRemoveModel = (providerName: string, modelId: string, event?: MouseEvent) => {
    event?.preventDefault();
    event?.stopPropagation();
    const ref = `${providerName}/${modelId}`;
    setPendingDelete({
      kind: "model",
      providerName,
      modelId,
      isPrimary: ref === primaryModel,
    });
  };

  const handleConfirmDelete = async () => {
    if (!pendingDelete) return;

    setDeleteBusy(true);
    try {
      const result: CommandResult = pendingDelete.kind === "provider"
        ? await invoke("delete_provider", { providerName: pendingDelete.providerName })
        : await invoke("remove_models_from_provider", {
            providerName: pendingDelete.providerName,
            modelIds: [pendingDelete.modelId],
          });

      if (!result.success) {
        alert(result.stderr || "删除失败");
        return;
      }

      setPendingDelete(null);
      await fetchData();
    } catch (error) {
      alert(`删除失败: ${error}`);
    } finally {
      setDeleteBusy(false);
    }
  };

  const handleFetchRemote = async () => {
    if (!addUrl || !addKey) return;
    setFetchingModels(true);
    setRemoteModels([]);
    try {
      const r: CommandResult = await invoke("fetch_remote_models", { baseUrl: addUrl, apiKey: addKey });
      if (r.success) {
        const ids: string[] = JSON.parse(r.stdout);
        setRemoteModels(ids);
        setSelectedModels(new Set(ids));
        if (!addName) {
          setAddName(addUrl.replace(/https?:\/\//, "").replace(/[:/.-]+/g, "-").slice(0, 30));
        }
        setView("sync");
      } else {
        alert(r.stderr || "获取模型失败");
      }
    } catch (e) { alert(`错误: ${e}`); }
    finally { setFetchingModels(false); }
  };

  const handleSync = async () => {
    if (selectedModels.size === 0) return;
    setSyncing(true);
    try {
      const r: CommandResult = await invoke("sync_models_to_provider", {
        providerName: addName, baseUrl: addUrl, apiKey: addKey,
        modelIds: Array.from(selectedModels),
      });
      if (r.success) {
        resetAddForm();
        await fetchData();
      } else {
        alert(r.stderr || "同步失败");
      }
    } catch (e) { alert(`错误: ${e}`); }
    finally { setSyncing(false); }
  };

  const handleStartRefreshProvider = async (provider: ProviderInfo) => {
    setRefreshingProviderName(provider.name);
    try {
      const result: CommandResult = await invoke("fetch_remote_models", {
        baseUrl: provider.base_url,
        apiKey: provider.api_key,
      });

      if (!result.success) {
        alert(result.stderr || "刷新失败");
        return;
      }

      const remoteIds: string[] = JSON.parse(result.stdout);
      const { candidates, selectedIds } = buildRefreshCandidates(provider, remoteIds);
      setRefreshProvider(provider);
      setRefreshCandidates(candidates);
      setRefreshSelectedModels(new Set(selectedIds));
      setView("refresh");
    } catch (error) {
      alert(`错误: ${error}`);
    } finally {
      setRefreshingProviderName("");
    }
  };

  const handleApplyRefresh = async () => {
    if (!refreshProvider) return;

    setRefreshApplying(true);
    try {
      const selectedIds = refreshCandidates
        .filter((candidate) => refreshSelectedModels.has(candidate.id))
        .map((candidate) => candidate.id);

      const result: CommandResult = await invoke("reconcile_provider_models", {
        providerName: refreshProvider.name,
        baseUrl: refreshProvider.base_url,
        apiKey: refreshProvider.api_key,
        selectedModelIds: selectedIds,
      });

      if (!result.success) {
        alert(result.stderr || "刷新失败");
        return;
      }

      resetAddForm();
      await fetchData();
      if (result.stdout) {
        alert(result.stdout);
      }
    } catch (error) {
      alert(`刷新失败: ${error}`);
    } finally {
      setRefreshApplying(false);
    }
  };

  const resetAddForm = () => {
    setView("list");
    setAddUrl("");
    setAddKey("");
    setAddName("");
    setRemoteModels([]);
    setSelectedModels(new Set());
    setRefreshingProviderName("");
    setRefreshApplying(false);
    setRefreshProvider(null);
    setRefreshCandidates([]);
    setRefreshSelectedModels(new Set());
  };

  const toggleModel = (id: string) => {
    setSelectedModels(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const toggleAll = () => {
    if (selectedModels.size === remoteModels.length) {
      setSelectedModels(new Set());
    } else {
      setSelectedModels(new Set(remoteModels));
    }
  };

  const toggleRefreshModel = (id: string) => {
    setRefreshSelectedModels((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const restoreCurrentRefreshSelection = () => {
    if (!refreshProvider) return;
    setRefreshSelectedModels(new Set(refreshProvider.models.map((model) => model.id)));
  };

  const selectAllRemoteRefreshModels = () => {
    setRefreshSelectedModels(
      new Set(
        refreshCandidates
          .filter((candidate) => candidate.status !== "missing")
          .map((candidate) => candidate.id),
      ),
    );
  };

  const selectAllRefreshModels = () => {
    setRefreshSelectedModels(new Set(refreshCandidates.map((candidate) => candidate.id)));
  };

  const totalModels = providers.reduce((sum, p) => sum + p.models.length, 0);
  const sortedProviders = [...providers].sort((a, b) => compareProviders(a, b, primaryModel));
  const refreshExistingIds = new Set(refreshProvider?.models.map((model) => model.id) ?? []);
  const refreshSelectedIds = refreshCandidates
    .filter((candidate) => refreshSelectedModels.has(candidate.id))
    .map((candidate) => candidate.id);
  const refreshAddCount = refreshSelectedIds.filter((id) => !refreshExistingIds.has(id)).length;
  const refreshRemoveCount = (refreshProvider?.models ?? []).filter((model) => !refreshSelectedModels.has(model.id)).length;
  const refreshMissingKeptCount = refreshCandidates.filter(
    (candidate) => candidate.status === "missing" && refreshSelectedModels.has(candidate.id),
  ).length;
  const refreshRemoteCount = refreshCandidates.filter((candidate) => candidate.status !== "missing").length;

  if (view === "add" || view === "sync" || view === "refresh") {
    return (
      <TooltipProvider delayDuration={300}>
        <ScrollArea className="flex-1">
          <div className="p-5 space-y-4">
            <div className="flex items-center justify-between">
              <h2 className="text-sm font-semibold">
                {view === "add" ? "添加 Provider" : view === "sync" ? "选择模型" : "刷新 Provider"}
              </h2>
              <Button size="sm" variant="ghost" onClick={resetAddForm}>
                <X /> 取消
              </Button>
            </div>

            {view === "add" && (
              <>
                <PresetGallery onSelect={applyPreset} />
                <Card>
                  <CardContent className="p-5 space-y-4">
                    <div className="rounded-xl border border-white/[0.06] bg-white/[0.03] px-3 py-2.5 text-[12px] text-muted-foreground">
                      先选少数常用预设即可，其他兼容服务直接手动填写 URL、Key 和 Provider 名称。
                    </div>
                    <div>
                      <label className="text-[12px] text-muted-foreground mb-1.5 block">API 地址</label>
                      <input className={inputCls} placeholder="http://host:port/v1" value={addUrl} onChange={(e) => setAddUrl(e.target.value)} />
                    </div>
                    <div>
                      <label className="text-[12px] text-muted-foreground mb-1.5 block">API Key</label>
                      <input className={inputCls} type="password" placeholder="sk-..." value={addKey} onChange={(e) => setAddKey(e.target.value)} />
                    </div>
                    <div>
                      <label className="text-[12px] text-muted-foreground mb-1.5 block">Provider 名称</label>
                      <input className={inputCls} placeholder="自动生成或自定义" value={addName} onChange={(e) => setAddName(e.target.value)} />
                    </div>
                    <div className="pt-1">
                      <Button size="sm" onClick={handleFetchRemote} disabled={!addUrl || !addKey || fetchingModels}>
                        {fetchingModels ? <Loader2 className="animate-spin" /> : <Globe />}
                        {fetchingModels ? "获取中..." : "获取模型列表"}
                      </Button>
                    </div>
                  </CardContent>
                </Card>
              </>
            )}

            {view === "sync" && (
              <>
                <Card>
                  <CardContent className="p-4">
                    <div className="flex items-center justify-between mb-3">
                      <div className="flex items-center gap-2">
                        <span className="text-[13px] font-medium">{addName}</span>
                        <Badge className="text-[10px] h-5 px-1.5 border-0 bg-teal-500/15 text-teal-400">
                          {selectedModels.size}/{remoteModels.length}
                        </Badge>
                      </div>
                      <Button size="sm" variant="ghost" onClick={toggleAll} className="text-xs h-6">
                        {selectedModels.size === remoteModels.length ? "取消全选" : "全选"}
                      </Button>
                    </div>
                    <div className="max-h-[400px] overflow-auto space-y-0.5">
                      {remoteModels.map((id) => (
                        <label
                          key={id}
                          className="flex items-center gap-2.5 px-2.5 py-2 rounded-lg hover:bg-white/[0.04] cursor-pointer transition-colors"
                          onClick={() => toggleModel(id)}
                        >
                          <div className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors ${
                            selectedModels.has(id) ? "bg-primary border-primary text-primary-foreground" : "border-white/[0.12]"
                          }`}>
                            {selectedModels.has(id) && <Check size={10} />}
                          </div>
                          <span className="text-[12px] font-mono text-foreground/80">{id}</span>
                        </label>
                      ))}
                    </div>
                  </CardContent>
                </Card>
                <div className="flex items-center gap-2">
                  <Button size="sm" onClick={handleSync} disabled={selectedModels.size === 0 || syncing}>
                    {syncing ? <Loader2 className="animate-spin" /> : <Plus />}
                    {syncing ? "同步中..." : `同步 ${selectedModels.size} 个模型`}
                  </Button>
                  <Button size="sm" variant="outline" onClick={() => setView("add")}>
                    返回修改
                  </Button>
                </div>
              </>
            )}

            {view === "refresh" && refreshProvider && (
              <>
                <Card>
                  <CardContent className="p-4 space-y-3">
                    <div className="flex flex-wrap items-center justify-between gap-3">
                      <div>
                        <div className="flex items-center gap-2">
                          <span className="text-[13px] font-medium">{refreshProvider.name}</span>
                          <Badge className="h-5 border-0 bg-violet-500/10 px-1.5 text-[10px] text-violet-300">
                            本地 {refreshProvider.models.length}
                          </Badge>
                          <Badge className="h-5 border-0 bg-teal-500/15 px-1.5 text-[10px] text-teal-300">
                            远端 {refreshRemoteCount}
                          </Badge>
                        </div>
                        <p className="mt-1 text-[11px] text-muted-foreground">
                          默认保留当前已配置模型。新增模型需要你勾选，标记为“远端已失效”的模型代表本地还在，但远端接口已经查不到。
                        </p>
                      </div>
                      <div className="rounded-lg border border-white/[0.06] bg-white/[0.03] px-3 py-2 text-[10px] text-muted-foreground">
                        <div className="uppercase tracking-widest">Provider URL</div>
                        <div className="mt-1 max-w-[360px] break-all font-mono text-foreground/70">
                          {refreshProvider.base_url}
                        </div>
                      </div>
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <Button type="button" size="sm" variant="outline" onClick={restoreCurrentRefreshSelection}>
                        保留当前配置
                      </Button>
                      <Button type="button" size="sm" variant="outline" onClick={selectAllRemoteRefreshModels}>
                        勾选全部远端模型
                      </Button>
                      <Button type="button" size="sm" variant="outline" onClick={selectAllRefreshModels}>
                        全部勾选
                      </Button>
                      <Button
                        type="button"
                        size="sm"
                        variant="ghost"
                        onClick={() => setRefreshSelectedModels(new Set())}
                      >
                        清空选择
                      </Button>
                    </div>
                  </CardContent>
                </Card>

                <Card>
                  <CardContent className="p-4">
                    <div className="mb-3 flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        <span className="text-[13px] font-medium">选择要保留的模型</span>
                        <Badge className="h-5 border-0 bg-sky-500/10 px-1.5 text-[10px] text-sky-300">
                          {refreshSelectedModels.size}/{refreshCandidates.length}
                        </Badge>
                      </div>
                      <span className="text-[11px] text-muted-foreground">
                        对齐逻辑参考 `syncModel`，你选中的会保留，没选中的会移除。
                      </span>
                    </div>

                    <div className="max-h-[420px] overflow-auto space-y-1">
                      {refreshCandidates.map((candidate) => {
                        const checked = refreshSelectedModels.has(candidate.id);
                        return (
                          <button
                            key={candidate.id}
                            type="button"
                            className={`flex w-full items-center gap-3 rounded-xl border px-3 py-2.5 text-left transition-colors ${
                              checked
                                ? "border-primary/30 bg-primary/[0.08]"
                                : "border-white/[0.06] bg-white/[0.02] hover:bg-white/[0.04]"
                            }`}
                            onClick={() => toggleRefreshModel(candidate.id)}
                          >
                            <div className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors ${
                              checked ? "border-primary bg-primary text-primary-foreground" : "border-white/[0.12]"
                            }`}>
                              {checked ? <Check size={10} /> : null}
                            </div>
                            <div className="min-w-0 flex-1">
                              <div className="flex items-center gap-2">
                                <span className="truncate font-mono text-[12px] text-foreground/85">{candidate.id}</span>
                                <RefreshStatusBadge status={candidate.status} />
                              </div>
                              <p className="mt-1 text-[11px] text-muted-foreground">
                                {describeRefreshCandidate(candidate.status)}
                              </p>
                            </div>
                          </button>
                        );
                      })}
                    </div>
                  </CardContent>
                </Card>

                <Card className="border-white/[0.06] bg-white/[0.02]">
                  <CardContent className="flex flex-wrap items-center gap-2.5 p-4">
                    <Badge className="h-5 border-0 bg-emerald-500/10 px-1.5 text-[10px] text-emerald-300">
                      +{refreshAddCount} 新增
                    </Badge>
                    <Badge className="h-5 border-0 bg-rose-500/10 px-1.5 text-[10px] text-rose-300">
                      -{refreshRemoveCount} 移除
                    </Badge>
                    {refreshMissingKeptCount > 0 && (
                      <Badge className="h-5 border-0 bg-amber-500/10 px-1.5 text-[10px] text-amber-300">
                        保留 {refreshMissingKeptCount} 个失效模型
                      </Badge>
                    )}
                    <div className="text-[12px] text-muted-foreground">
                      应用后会同步更新 Provider 模型列表，并修正默认模型映射。
                    </div>
                  </CardContent>
                </Card>

                <div className="flex items-center gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant={refreshSelectedModels.size === 0 ? "destructive" : "default"}
                    onClick={() => void handleApplyRefresh()}
                    disabled={refreshApplying}
                  >
                    {refreshApplying ? <Loader2 className="animate-spin" /> : <RefreshCw />}
                    {refreshApplying
                      ? "应用中..."
                      : refreshSelectedModels.size === 0
                        ? "清空这个 Provider"
                        : `应用刷新（保留 ${refreshSelectedModels.size} 个）`}
                  </Button>
                  <Button type="button" size="sm" variant="outline" onClick={resetAddForm} disabled={refreshApplying}>
                    返回列表
                  </Button>
                </div>
              </>
            )}
          </div>
        </ScrollArea>
      </TooltipProvider>
    );
  }

  return (
    <TooltipProvider delayDuration={300}>
      <ScrollArea className="flex-1">
        <div className="p-5 space-y-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2.5">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-violet-500/10">
                <Box size={15} className="text-violet-400" />
              </div>
              <div>
                <h2 className="text-sm font-semibold">模型管理</h2>
                <p className="text-[11px] text-muted-foreground">
                  {loading ? "加载中" : `${providers.length} Provider · ${totalModels} 模型`}
                </p>
              </div>
            </div>
            <div className="flex items-center gap-1.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" size="icon" className="h-7 w-7" onClick={fetchData} disabled={loading}>
                    {loading ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
                  </Button>
                </TooltipTrigger>
                <TooltipContent>刷新</TooltipContent>
              </Tooltip>
              <Button size="sm" onClick={() => setView("add")}>
                <Plus /> 添加 Provider
              </Button>
            </div>
          </div>

          {primaryModel && (
            <Card className="border-amber-500/15">
              <CardContent className="flex items-center gap-3 p-4">
                <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-amber-500/10">
                  <Star size={16} className="text-amber-400" />
                </div>
                <div className="flex-1 min-w-0">
                  <p className="text-[10px] uppercase tracking-widest text-muted-foreground">主模型</p>
                  <p className="text-[14px] font-semibold font-mono truncate">{primaryModel}</p>
                </div>
              </CardContent>
            </Card>
          )}

          {!loading && providers.length > 0 && (
            <Card className="border-white/[0.06] bg-white/[0.02]">
              <CardContent className="flex flex-wrap items-center gap-2.5 p-4">
                <div className="text-[12px] text-muted-foreground">
                  已配置内容会优先显示在这里。新增 Provider 时再选择预设，页面不会把常用操作挤到下面。
                </div>
                <Button size="sm" variant="outline" onClick={() => setView("add")}>
                  <Plus /> 新增一个兼容入口
                </Button>
              </CardContent>
            </Card>
          )}

          {loading ? (
            <div className="flex items-center justify-center py-20 text-muted-foreground">
              <Loader2 size={18} className="animate-spin mr-2" />
              <span className="text-[13px]">加载中...</span>
            </div>
          ) : providers.length === 0 ? (
            <EmptyState onAdd={() => setView("add")} />
          ) : (
            <div className="space-y-3">
              {sortedProviders.map((p) => (
                <ProviderCard
                  key={p.name}
                  provider={p}
                  primaryModel={primaryModel}
                  onDelete={() => handleDeleteProvider(p.name)}
                  onSetPrimary={handleSetPrimary}
                  onRemoveModel={(mid) => handleRemoveModel(p.name, mid)}
                  onRefresh={() => void handleStartRefreshProvider(p)}
                  refreshing={refreshingProviderName === p.name}
                  settingPrimaryRef={settingPrimaryRef}
                />
              ))}
            </div>
          )}
        </div>
      </ScrollArea>
      <ConfirmActionDialog
        open={Boolean(pendingDelete)}
        title={
          pendingDelete?.kind === "provider"
            ? `删除 Provider “${pendingDelete.providerName}”？`
            : `移除模型 “${pendingDelete?.modelId}”？`
        }
        description={
          pendingDelete?.kind === "provider"
            ? "这会删除这个 Provider 以及它下面的全部模型配置。确认后才会真正执行。"
            : pendingDelete?.isPrimary
              ? "这个模型当前正被设为主模型。移除后会自动切换到下一个可用模型。确认后才会真正执行。"
              : "这个模型会从当前 Provider 中移除。确认后才会真正执行。"
        }
        confirmLabel={pendingDelete?.kind === "provider" ? "确认删除" : "确认移除"}
        destructive
        busy={deleteBusy}
        onCancel={() => {
          if (!deleteBusy) {
            setPendingDelete(null);
          }
        }}
        onConfirm={() => void handleConfirmDelete()}
      />
    </TooltipProvider>
  );
}

function ProviderCard({ provider, primaryModel, onDelete, onSetPrimary, onRemoveModel, onRefresh, refreshing, settingPrimaryRef }: {
  provider: ProviderInfo;
  primaryModel: string;
  onDelete: () => void;
  onSetPrimary: (ref: string) => Promise<void>;
  onRemoveModel: (modelId: string) => void;
  onRefresh: () => void;
  refreshing: boolean;
  settingPrimaryRef: string;
}) {
  const hasPrimaryModel = provider.models.some((model) => `${provider.name}/${model.id}` === primaryModel);
  const [expanded, setExpanded] = useState(hasPrimaryModel);
  const sortedModels = [...provider.models].sort((a, b) => compareModels(provider.name, a, b, primaryModel));

  const maskedKey = provider.api_key
    ? `${provider.api_key.slice(0, 6)}...${provider.api_key.slice(-4)}`
    : "";

  return (
    <Card>
      <CardContent className="p-0">
        <div
          role="button"
          tabIndex={0}
          className="flex items-center gap-3 w-full p-4 text-left hover:bg-white/[0.02] transition-colors cursor-pointer"
          onClick={() => setExpanded(!expanded)}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              setExpanded(!expanded);
            }
          }}
        >
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-violet-500/10">
            <Globe size={16} className="text-violet-400" />
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <span className="text-[13px] font-medium">{provider.name}</span>
              <Badge className="text-[10px] h-4 px-1.5 border-0 bg-violet-500/10 text-violet-400">
                {provider.models.length} 模型
              </Badge>
              {hasPrimaryModel && (
                <Badge className="text-[10px] h-4 px-1.5 border-0 bg-amber-500/15 text-amber-400">
                  当前主模型
                </Badge>
              )}
            </div>
            <div className="flex items-center gap-2 mt-0.5">
              <span className="text-[11px] text-muted-foreground font-mono truncate">{provider.base_url}</span>
              {maskedKey && (
                <>
                  <span className="text-white/10">·</span>
                  <span className="text-[10px] text-muted-foreground font-mono flex items-center gap-0.5">
                    <Key size={9} />{maskedKey}
                  </span>
                </>
              )}
            </div>
          </div>
          <div className="flex items-center gap-1 shrink-0" onClick={(e) => e.stopPropagation()}>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-6 w-6"
                  onMouseDown={(event) => event.stopPropagation()}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onRefresh();
                  }}
                  disabled={refreshing}
                >
                  {refreshing ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
                </Button>
              </TooltipTrigger>
              <TooltipContent>刷新模型</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-6 w-6 text-muted-foreground hover:text-destructive"
                  onMouseDown={(event) => event.stopPropagation()}
                  onClick={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    onDelete();
                  }}
                >
                  <Trash2 size={12} />
                </Button>
              </TooltipTrigger>
              <TooltipContent>删除 Provider</TooltipContent>
            </Tooltip>
          </div>
          {expanded ? <ChevronUp size={14} className="text-muted-foreground" /> : <ChevronDown size={14} className="text-muted-foreground" />}
        </div>

        {expanded && provider.models.length > 0 && (
          <div className="border-t border-white/[0.06]">
            <div className="flex items-center justify-between gap-3 px-4 py-2.5 text-[11px] text-muted-foreground">
              <span>主模型会固定显示在最前面，右侧按钮可直接切换。</span>
              <span>{provider.models.length} 个模型</span>
            </div>
            <div className="max-h-[300px] overflow-auto">
              {sortedModels.map((m) => {
                const ref = `${provider.name}/${m.id}`;
                const isPrimary = ref === primaryModel;
                const isSettingPrimary = ref === settingPrimaryRef;
                return (
                  <div
                    key={m.id}
                    className={`px-4 py-2.5 text-[12px] transition-colors ${
                      isPrimary ? "bg-amber-500/6" : "hover:bg-white/[0.02]"
                    }`}
                  >
                    <div className="flex items-center gap-2.5">
                      <Cpu size={11} className="text-muted-foreground shrink-0" />
                      <span className={`font-mono flex-1 min-w-0 truncate ${isPrimary ? "text-foreground" : "text-foreground/80"}`}>{m.id}</span>
                      {m.reasoning && <Badge className="text-[9px] h-3.5 px-1 border-0 bg-amber-500/15 text-amber-400">R</Badge>}
                      {m.input.includes("image") && <Badge className="text-[9px] h-3.5 px-1 border-0 bg-sky-500/15 text-sky-400">V</Badge>}
                      <span className="text-[10px] text-muted-foreground/50 w-14 text-right shrink-0 font-mono">{(m.context_window / 1000).toFixed(0)}k</span>
                    </div>
                    <div className="mt-2 flex items-center justify-between gap-3">
                      <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
                        {isPrimary ? (
                          <>
                            <Star size={11} className="text-amber-400 shrink-0" />
                            <span className="text-amber-300">当前主模型</span>
                          </>
                        ) : (
                          <span>可设为当前主模型</span>
                        )}
                      </div>
                      <div className="flex items-center gap-2">
                        {isPrimary ? (
                          <Badge className="border-0 bg-amber-500/15 text-[10px] text-amber-400">
                            正在使用
                          </Badge>
                        ) : (
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          className="h-7 text-[11px]"
                          disabled={Boolean(settingPrimaryRef)}
                            onClick={async (event) => {
                              event.stopPropagation();
                              await onSetPrimary(ref);
                            }}
                          >
                            {isSettingPrimary ? <Loader2 size={11} className="animate-spin" /> : <Star size={11} />}
                            {isSettingPrimary ? "设置中..." : "设为主模型"}
                          </Button>
                        )}
                        <Button
                          type="button"
                          size="sm"
                          variant="ghost"
                          className="h-7 text-[11px] text-muted-foreground hover:text-destructive"
                          onClick={(event) => {
                            event.preventDefault();
                            event.stopPropagation();
                            onRemoveModel(m.id);
                          }}
                        >
                          <X size={11} />
                          移除
                        </Button>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function EmptyState({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center py-20 text-center">
      <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-white/[0.04] mb-4">
        <Box size={24} className="text-muted-foreground" />
      </div>
      <h3 className="text-sm font-medium mb-1">还没有配置模型</h3>
      <p className="text-[12px] text-muted-foreground mb-5 max-w-[260px]">
        先选一个常用入口，或者手动添加 OpenAI 兼容 URL 来同步模型。
      </p>
      <Button size="sm" onClick={onAdd}>
        <Plus /> 添加 Provider
      </Button>
    </div>
  );
}

function PresetGallery({ onSelect, compact = false }: {
  onSelect: (preset: ProviderPreset) => void;
  compact?: boolean;
}) {
  return (
    <Card className={compact ? "border-white/[0.06]" : ""}>
      <CardContent className="p-4">
        <div className="flex items-center justify-between gap-3 mb-3">
          <div>
            <h3 className="text-[13px] font-semibold">常用 Provider 预设</h3>
            <p className="text-[11px] text-muted-foreground mt-1">
              保留最常用的 3 个入口，其余兼容服务建议直接手动填写。
            </p>
          </div>
          <Badge className="border-0 bg-violet-500/10 text-violet-400 text-[10px]">
            OpenAI-compatible
          </Badge>
        </div>

        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {PROVIDER_PRESETS.map((preset) => (
            <button
              key={preset.id}
              type="button"
              onClick={() => onSelect(preset)}
              className="rounded-xl border border-white/[0.06] bg-white/[0.03] p-3 text-left transition-colors hover:border-violet-500/20 hover:bg-white/[0.05]"
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-[13px] font-medium text-foreground/90">{preset.title}</span>
                <Badge className="h-5 border-0 bg-white/[0.06] text-[10px] text-muted-foreground">
                  {preset.badge}
                </Badge>
              </div>
              <p className="mt-2 text-[11px] leading-relaxed text-muted-foreground">{preset.description}</p>
              <p className="mt-3 text-[10px] font-mono text-foreground/60 truncate">{preset.baseUrl}</p>
            </button>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}

function compareProviders(a: ProviderInfo, b: ProviderInfo, primaryModel: string) {
  const aHasPrimary = a.models.some((model) => `${a.name}/${model.id}` === primaryModel);
  const bHasPrimary = b.models.some((model) => `${b.name}/${model.id}` === primaryModel);

  if (aHasPrimary !== bHasPrimary) {
    return aHasPrimary ? -1 : 1;
  }

  if (a.models.length !== b.models.length) {
    return b.models.length - a.models.length;
  }

  return a.name.localeCompare(b.name);
}

function compareModels(providerName: string, a: ProviderInfo["models"][number], b: ProviderInfo["models"][number], primaryModel: string) {
  const aIsPrimary = `${providerName}/${a.id}` === primaryModel;
  const bIsPrimary = `${providerName}/${b.id}` === primaryModel;

  if (aIsPrimary !== bIsPrimary) {
    return aIsPrimary ? -1 : 1;
  }

  if (a.reasoning !== b.reasoning) {
    return a.reasoning ? -1 : 1;
  }

  return a.id.localeCompare(b.id);
}

function buildRefreshCandidates(provider: ProviderInfo, remoteIds: string[]) {
  const currentIds = provider.models.map((model) => model.id);
  const seenRemote = new Set<string>();
  const candidates: RefreshCandidate[] = [];

  for (const id of remoteIds) {
    if (!id || seenRemote.has(id)) {
      continue;
    }
    seenRemote.add(id);
    candidates.push({
      id,
      status: currentIds.includes(id) ? "existing" : "new",
    });
  }

  for (const id of currentIds) {
    if (!seenRemote.has(id)) {
      candidates.push({ id, status: "missing" });
    }
  }

  return {
    candidates,
    selectedIds: currentIds,
  };
}

function describeRefreshCandidate(status: RefreshCandidateStatus) {
  switch (status) {
    case "new":
      return "远端新发现的模型，勾选后会追加到当前 Provider。";
    case "missing":
      return "本地已有，但这次远端接口没有返回；如果不再需要，可以取消勾选。";
    case "existing":
    default:
      return "当前 Provider 已经包含这个模型，默认继续保留。";
  }
}

function RefreshStatusBadge({ status }: { status: RefreshCandidateStatus }) {
  if (status === "new") {
    return (
      <Badge className="h-4 border-0 bg-emerald-500/10 px-1.5 text-[10px] text-emerald-300">
        新模型
      </Badge>
    );
  }

  if (status === "missing") {
    return (
      <Badge className="h-4 border-0 bg-amber-500/10 px-1.5 text-[10px] text-amber-300">
        远端已失效
      </Badge>
    );
  }

  return (
    <Badge className="h-4 border-0 bg-sky-500/10 px-1.5 text-[10px] text-sky-300">
      已配置
    </Badge>
  );
}
