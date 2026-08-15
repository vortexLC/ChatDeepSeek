import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  clearAllConversations,
  createConversation,
  deleteConversation,
  editAndResend,
  fetchFileContent,
  fetchWebPage,
  getArtifactAbsPath,
  getContextStatus,
  getInitialState,
  getMessages,
  listConversations,
  onChatEvent,
  respondPermission,
  saveSettings,
  sendMessage,
  stopGeneration,
  updateConversation,
} from "./api";
import type { ConversationUpdate } from "./api";
import type {
  AgentMode,
  AppSettings,
  Artifact,
  ChatDraft,
  ChatEventPayload,
  ChatStatus,
  ContextStatus,
  Conversation,
  EditTarget,
  Effort,
  Message,
  ModelOption,
  PermissionRequest,
  PreviewContent,
  ToolStep,
  UploadAttachment,
  WebPage,
} from "./types";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { SettingsPanel } from "./components/SettingsPanel";
import { WebPreviewPanel } from "./components/WebPreviewPanel";
import { ToastStack, type ToastItem, type ToastType } from "./components/Toast";
import { XIcon } from "./components/icons";

const DEFAULT_SETTINGS: AppSettings = {
  theme: "auto",
  default_web_search: false,
  default_deep_think: false,
  default_effort: "high",
  default_model: "deepseek-v4-flash",
  default_mode: "chat",
  deepseek: {
    api_key: "",
  },
  search: {
    tavily_key: "",
    tavily_enabled: true,
    anysearch_key: "",
    anysearch_enabled: true,
    strategy: "auto",
    max_results: 5,
  },
  gen: {
    provider: "siliconflow",
    siliconflow: {
      api_key: "",
      base_url: "https://api.siliconflow.cn/v1",
      image_model: "Kwai-Kolors/Kolors",
    },
    alibaba: {
      api_key: "",
      base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
      image_model: "wanx-v1",
    },
  },
  providers: [
    {
      id: "deepseek",
      name: "DeepSeek 官方",
      protocol: "openai",
      api_base: "https://api.deepseek.com/v1",
      api_key: "",
      models: [
        { id: "m_flash", name: "deepseek-v4-flash", model_type: "text", context_tokens: 131072 },
        { id: "m_pro", name: "deepseek-v4-pro", model_type: "text", context_tokens: 131072 },
      ],
    },
  ],
  chat_model: { provider_id: "deepseek", model_id: "m_flash" },
  image_model: null,
};

function emptyDraft(): ChatDraft {
  return {
    status: "idle",
    reasoning: "",
    content: "",
    searchItems: [],
    artifacts: [],
    steps: [],
    searchProvider: null,
    error: null,
  };
}

function applyTheme(theme: AppSettings["theme"]) {
  const dark =
    theme === "dark" ||
    (theme === "auto" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

export default function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [loadingMessages, setLoadingMessages] = useState(false);
  const [drafts, setDrafts] = useState<Record<number, ChatDraft>>({});
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const toastSeqRef = useRef(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [errorBanner, setErrorBanner] = useState<string | null>(null);
  const [context, setContext] = useState<ContextStatus | null>(null);
  const [preview, setPreview] = useState<PreviewContent | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [editTarget, setEditTarget] = useState<EditTarget | null>(null);
  const [permRequest, setPermRequest] = useState<PermissionRequest | null>(null);
  const activeIdRef = useRef<number | null>(null);
  const errorTimerRef = useRef<number | null>(null);
  /** 当前会话请求序号：异步响应（消息/上下文）返回时校验，防止切换会话后旧响应覆盖新会话 */
  const convReqRef = useRef<number | null>(null);
  /** 预览请求序号：快速连续点击链接时，丢弃先请求后返回的过期响应 */
  const previewSeqRef = useRef(0);
  activeIdRef.current = activeId;

  const showError = useCallback((msg: string) => {
    setErrorBanner(msg);
    if (errorTimerRef.current !== null) {
      window.clearTimeout(errorTimerRef.current);
    }
    errorTimerRef.current = window.setTimeout(() => {
      setErrorBanner(null);
      errorTimerRef.current = null;
    }, 5000);
  }, []);

  const dismissError = useCallback(() => {
    if (errorTimerRef.current !== null) {
      window.clearTimeout(errorTimerRef.current);
      errorTimerRef.current = null;
    }
    setErrorBanner(null);
  }, []);

  /** 右上角堆叠通知：异步任务提交/完成/失败 的即时反馈，约 4.5s 自动消失 */
  const pushToast = useCallback((type: ToastType, text: string) => {
    const id = ++toastSeqRef.current;
    setToasts((prev) => [...prev, { id, type, text }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4500);
  }, []);

  const dismissToast = useCallback((id: number) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const activeConversation = useMemo(
    () => conversations.find((c) => c.id === activeId) ?? null,
    [conversations, activeId]
  );
  const activeDraft = activeId ? (drafts[activeId] ?? null) : null;

  useEffect(() => {
    applyTheme(settings.theme);
  }, [settings.theme]);

  useEffect(() => {
    let mounted = true;
    getInitialState()
      .then((state) => {
        if (!mounted) return;
        setConversations(state.conversations);
        setSettings({ ...DEFAULT_SETTINGS, ...state.settings });
      })
      .catch((e) => showError(`初始化失败: ${e}`));
    return () => {
      mounted = false;
      if (errorTimerRef.current !== null) {
        window.clearTimeout(errorTimerRef.current);
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (settings.theme === "auto") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const handler = () => applyTheme("auto");
      mq.addEventListener("change", handler);
      return () => mq.removeEventListener("change", handler);
    }
  }, [settings.theme]);

  const reloadConversations = useCallback(async () => {
    try {
      // 仅刷新会话列表，避免重复拉取完整 settings
      setConversations(await listConversations());
    } catch {
      /* ignore */
    }
  }, []);

  /** 拉取会话消息。返回是否成功应用：失败时保留当前视图与流式气泡，
   *  避免已生成的内容从界面消失（可稍后切换会话恢复） */
  const reloadMessages = useCallback(async (id: number): Promise<boolean> => {
    convReqRef.current = id;
    setLoadingMessages(true);
    try {
      const list = await getMessages(id);
      if (convReqRef.current !== id) return false; // 已切换会话，丢弃过期响应
      setMessages(list);
      return true;
    } catch {
      return false;
    } finally {
      if (convReqRef.current === id) setLoadingMessages(false);
    }
  }, []);

  const refreshContext = useCallback(async (id: number) => {
    try {
      const ctx = await getContextStatus(id);
      if (convReqRef.current !== id) return; // 已切换会话，丢弃过期响应
      setContext(ctx);
    } catch {
      /* ignore */
    }
  }, []);

  const handleChatEvent = useCallback(
    (payload: ChatEventPayload) => {
      const cid = payload.conversation_id;
      setDrafts((prev) => {
        const cur = prev[cid] ?? emptyDraft();
        let next: ChatDraft = cur;
        switch (payload.kind) {
        case "status":
          next = {
            ...cur,
            status:
              (payload.text as ChatStatus) === "searching"
                ? "searching"
                : (payload.text as ChatStatus) === "analyzing"
                  ? "analyzing"
                  : (payload.text as ChatStatus) === "answering"
                    ? "answering"
                    : (payload.text as ChatStatus) === "generating"
                      ? "generating"
                      : "thinking",
            searchProvider: payload.search_provider ?? cur.searchProvider,
          };
          break;
          case "reasoning_delta":
            next = { ...cur, reasoning: cur.reasoning + (payload.text ?? "") };
            break;
          case "content_delta":
            next = { ...cur, content: cur.content + (payload.text ?? "") };
            break;
        case "search_result":
          if (payload.item && "url" in payload.item) {
            next = { ...cur, searchItems: [...cur.searchItems, payload.item] };
          }
          break;
        case "tool_step":
          // 执行时间线步骤（思考/工具调用完成）：payload.text 为 ToolStep JSON
          if (payload.text) {
            try {
              const step = JSON.parse(payload.text) as ToolStep;
              next = { ...cur, steps: [...cur.steps, step] };
            } catch {
              /* 忽略无法解析的步骤 */
            }
          }
          break;
        case "artifact":
          if (payload.item && "path" in payload.item) {
            next = { ...cur, artifacts: [...cur.artifacts, payload.item as Artifact] };
          }
          break;
        case "permission_request":
        case "stopped":
          // 非流式事件：不创建/保留空 draft，避免完成后凭空出现空白气泡
          return prev;
          case "error":
            next = { ...cur, error: payload.text ?? "生成失败" };
            break;
          case "done":
            return prev;
        }
        return { ...prev, [cid]: next };
      });

      if (payload.kind === "done") {
        // 先等持久化内容加载回来再移除流式气泡，避免回复短暂"消失"；
        // 加载失败时保留气泡（内容仍在界面上，可稍后切换会话恢复）
        const removeDraft = () => {
          setDrafts((prev) => {
            const { [cid]: _removed, ...rest } = prev;
            return rest;
          });
        };
        reloadConversations();
        if (activeIdRef.current === cid) {
          reloadMessages(cid).then((ok) => {
            if (ok) removeDraft();
            refreshContext(cid);
          });
        } else {
          removeDraft();
        }
      }
    if (payload.kind === "permission_request") {
      setPermRequest({
        conversation_id: payload.conversation_id,
        tool: payload.tool ?? "",
        path: payload.path ?? "",
      });
      // 与后端 90 秒超时保持一致：超时后自动关闭弹窗，避免悬挂
      window.setTimeout(() => setPermRequest(null), 90000);
    }
    if (payload.kind === "stopped") {
      // 用户主动停止：非错误。先等持久化的部分内容加载回来，再移除流式气泡，
      // 避免已生成内容短暂闪现消失；加载失败时保留气泡（内容仍在，可稍后恢复）
      const removeDraft = () => {
        setDrafts((prev) => {
          const { [cid]: _removed, ...rest } = prev;
          return rest;
        });
      };
      if (activeIdRef.current === cid) {
        reloadMessages(cid).then((ok) => {
          if (ok) removeDraft();
          refreshContext(cid);
        });
      } else {
        removeDraft();
      }
    }
    if (payload.kind === "error") {
        // 仅当前活动会话的错误弹横幅；后台会话的错误只清理其 draft，避免误导
        const removeDraft = () => {
          setDrafts((prev) => {
            const { [cid]: _removed, ...rest } = prev;
            return rest;
          });
        };
        if (activeIdRef.current === cid) {
          showError(payload.text ?? "生成失败，请检查 API 配置");
          // 先等持久化的部分内容加载回来再移除气泡，避免已生成内容随错误消失；
          // 加载失败时保留气泡（内容仍在界面上）
          reloadMessages(cid).then((ok) => {
            if (ok) removeDraft();
            refreshContext(cid);
          });
        } else {
          removeDraft();
        }
      }
    },
    [reloadConversations, reloadMessages, refreshContext, showError, pushToast]
  );

  useEffect(() => {
    const unlistenPromise = onChatEvent(handleChatEvent);
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [handleChatEvent]);

  const selectConversation = useCallback(
    async (id: number) => {
      setActiveId(id);
      setMessages([]);
      setContext(null);
      setEditTarget(null);
      // 清理该会话残留的流式草稿（如上次 reload 失败时保留的），
      // 避免与消息列表中已持久化的内容重复展示；若会话仍在生成，
      // 后续事件会基于 emptyDraft 重新创建草稿，不影响继续流式输出
      setDrafts((prev) => {
        const { [id]: _removed, ...rest } = prev;
        return rest;
      });
      reloadMessages(id);
      refreshContext(id);
    },
    [reloadMessages, refreshContext]
  );

  const newConversation = useCallback(async () => {
    try {
      const conv = await createConversation();
      setConversations((prev) => [conv, ...prev]);
      setActiveId(conv.id);
      setMessages([]);
      setContext(null);
      setEditTarget(null);
      refreshContext(conv.id);
    } catch (e) {
      showError(`新建对话失败: ${e}`);
    }
  }, [refreshContext]);

  const renameConversation = useCallback(
    (id: number, title: string) => {
      setConversations((prev) =>
        prev.map((c) => (c.id === id ? { ...c, title } : c))
      );
      updateConversation(id, { title }).catch(() => {});
    },
    []
  );

  const removeConversation = useCallback(
    async (id: number) => {
      try {
        await deleteConversation(id);
      } catch (e) {
        // 删除失败（目录被占用等）：保留会话并提示，避免数据残留却从列表消失
        showError(`删除对话失败: ${e}`);
        return;
      }
      setConversations((prev) => prev.filter((c) => c.id !== id));
      setDrafts((prev) => {
        const { [id]: _removed, ...rest } = prev;
        return rest;
      });
      if (activeId === id) {
        setActiveId(null);
        setMessages([]);
        setContext(null);
        setEditTarget(null);
      }
    },
    [activeId, showError]
  );

  const patchConversation = useCallback(
    (patch: ConversationUpdate) => {
      if (!activeId || !activeConversation) return;
      const normalized: ConversationUpdate = { ...patch };
      setConversations((prev) =>
        prev.map((c) => (c.id === activeId ? { ...c, ...normalized } : c))
      );
      updateConversation(activeId, normalized).catch(() => {});
    },
    [activeId, activeConversation]
  );

  const handleSend = useCallback(
    async (content: string, attachments: UploadAttachment[] = []): Promise<boolean> => {
      if (!activeId) return false;
      if (content.trim() === "" && attachments.length === 0) return false;
      if (context?.full) {
        showError("上下文已满，请新开会话");
        return false;
      }
      const tempUser: Message = {
        id: -Date.now(),
        conversation_id: activeId,
        role: "user",
        content,
        reasoning: "",
        search_results: [],
        artifacts: [],
        steps: [],
        attachments: attachments.map((a) => ({
          name: a.name,
          mime: a.mime,
          kind: a.kind,
          path: a.data_url,
          size: 0,
        })),
        created_at: Date.now(),
      };
      setMessages((prev) => [...prev, tempUser]);
      setDrafts((prev) => ({ ...prev, [activeId]: emptyDraft() }));
      try {
        await sendMessage(activeId, content, attachments);
        return true;
      } catch (e) {
        showError(`发送失败: ${e}`);
        setMessages((prev) => prev.filter((m) => m.id !== tempUser.id));
        setDrafts((prev) => {
          const { [activeId]: _removed, ...rest } = prev;
          return rest;
        });
        return false;
      }
    },
    [activeId, context]
  );

  const openWebPreview = useCallback(async (url: string) => {
    const seq = ++previewSeqRef.current;
    setPreviewOpen(true);
    setPreviewError(null);
    setPreviewLoading(true);
    setPreview({ kind: "web", url, title: "", html: "" });
    try {
      const page: WebPage = await fetchWebPage(url);
      if (seq !== previewSeqRef.current) return; // 已点击其它链接，丢弃过期响应
      setPreview({ kind: "web", url, title: page.title, html: page.html });
    } catch (e) {
      if (seq !== previewSeqRef.current) return;
      showError(`预览加载失败: ${e}`);
      setPreviewError(String(e));
    } finally {
      if (seq === previewSeqRef.current) setPreviewLoading(false);
    }
  }, [showError]);

  const openFilePreview = useCallback(
    async (convId: number, path: string, title: string) => {
      const seq = ++previewSeqRef.current;
      setPreviewOpen(true);
      setPreviewError(null);
      setPreviewLoading(true);
      setPreview({ kind: "file", url: path, title, html: "" });
      try {
        const page: WebPage = await fetchFileContent(convId, path);
        if (seq !== previewSeqRef.current) return;
        setPreview({ kind: "file", url: page.url, title: page.title, html: page.html });
      } catch (e) {
        if (seq !== previewSeqRef.current) return;
        showError(`文件预览失败: ${e}`);
        setPreviewError(String(e));
      } finally {
        if (seq === previewSeqRef.current) setPreviewLoading(false);
      }
    },
    [showError]
  );

  const openMediaPreview = useCallback(
    async (convId: number, artifact: Artifact) => {
      const seq = ++previewSeqRef.current;
      setPreviewOpen(true);
      setPreviewError(null);
      setPreviewLoading(false);
      try {
        const abs = await getArtifactAbsPath(convId, artifact.path);
        if (seq !== previewSeqRef.current) return;
        setPreview({
          kind: artifact.kind as "image",
          url: convertFileSrc(abs),
          title: artifact.name,
          html: "",
        });
      } catch (e) {
        if (seq !== previewSeqRef.current) return;
        showError(`预览失败: ${e}`);
        setPreviewError(String(e));
      }
    },
    [showError]
  );

  const togglePreview = useCallback(() => {
    setPreviewOpen((v) => !v);
  }, []);

  const handleEditMessage = useCallback((m: Message) => {
    setEditTarget({ id: m.id, text: m.content });
  }, []);

  const handleCancelEdit = useCallback(() => {
    setEditTarget(null);
  }, []);

  const handleSendEdit = useCallback(
    async (content: string, attachments: UploadAttachment[] = []): Promise<boolean> => {
      if (!activeId || !editTarget) return false;
      const tempUser: Message = {
        id: -Date.now(),
        conversation_id: activeId,
        role: "user",
        content,
        reasoning: "",
        search_results: [],
        artifacts: [],
        steps: [],
        attachments: attachments.map((a) => ({
          name: a.name,
          mime: a.mime,
          kind: a.kind,
          path: a.data_url,
          size: 0,
        })),
        created_at: Date.now(),
      };
      setMessages((prev) => {
        const idx = prev.findIndex((m) => m.id === editTarget.id);
        if (idx < 0) return [...prev, tempUser];
        return [...prev.slice(0, idx), tempUser];
      });
      setDrafts((prev) => ({ ...prev, [activeId]: emptyDraft() }));
      try {
        await editAndResend(activeId, editTarget.id, content, attachments);
        setEditTarget(null);
        return true;
      } catch (e) {
        showError(`发送失败: ${e}`);
        // 回滚被截断的历史消息（重新拉取），避免编辑点之后的消息从界面"变短"
        reloadMessages(activeId);
        setDrafts((prev) => {
          const { [activeId]: _removed, ...rest } = prev;
          return rest;
        });
        return false;
      }
    },
    [activeId, editTarget]
  );

  // 对话模型选项：所有已添加的文本/多模态模型（按提供商分组）
  const chatModelOptions = useMemo(() => {
    const options: ModelOption[] = [];
    for (const p of settings.providers) {
      for (const m of p.models) {
        if (m.model_type === "text" || m.model_type === "vision") {
          options.push({
            label: `${p.name} / ${m.name}`,
            model: m.id,
            modelType: m.model_type,
            protocol: p.protocol,
          });
        }
      }
    }
    return options;
  }, [settings.providers]);

  // 默认对话模型：与后端 resolve_chat_model 保持一致
  // （设置 → 模型选择 中的对话模型优先，否则取第一个文本/视觉模型），
  // 用于在会话未单独选择模型时让发送框显示与设置一致的模型
  const defaultChatModelId = useMemo(() => {
    const sel = settings.chat_model;
    if (sel) {
      const p = settings.providers.find((x) => x.id === sel.provider_id);
      const m = p?.models.find((mm) => mm.id === sel.model_id);
      if (p && m && (m.model_type === "text" || m.model_type === "vision")) {
        return m.id;
      }
    }
    return chatModelOptions[0]?.model ?? "";
  }, [settings.providers, settings.chat_model, chatModelOptions]);

  const handleSaveSettings = useCallback(async (next: AppSettings) => {
    setSettings(next);
    try {
      await saveSettings(next);
    } catch (e) {
      showError(`保存设置失败: ${e}`);
    }
  }, []);

  const handleClearAll = useCallback(async () => {
    try {
      await clearAllConversations();
      setConversations([]);
      setActiveId(null);
      setMessages([]);
      setDrafts({});
      setContext(null);
      setEditTarget(null);
      setPreview(null);
      setPreviewOpen(false);
    } catch (e) {
      showError(`清空失败: ${e}`);
    }
  }, []);

  return (
    <div className="app">
      <Sidebar
        conversations={conversations}
        activeId={activeId}
        onSelect={selectConversation}
        onNew={newConversation}
        onDelete={removeConversation}
        onRename={renameConversation}
        onOpenSettings={() => setSettingsOpen(true)}
      />
      <ChatView
        conversation={activeConversation}
        messages={messages}
        loadingMessages={loadingMessages}
        draft={activeDraft}
        context={context}
        onNewConversation={newConversation}
        previewOpen={previewOpen}
        onTogglePreview={togglePreview}
        modelOptions={chatModelOptions}
        defaultModel={defaultChatModelId}
        onSelectModel={(option) => {
          const p = settings.providers.find((x) =>
            x.models.some((m) => m.id === option.model)
          );
          if (!p) return;
          patchConversation({ provider: p.id, model: option.model });
        }}
        onToggleWebSearch={() =>
          patchConversation({
            web_search: !(activeConversation?.web_search ?? false),
          })
        }
        onToggleDeepThink={() =>
          patchConversation({
            deep_think: !(activeConversation?.deep_think ?? false),
          })
        }
        onSetEffort={(effort: Effort) => patchConversation({ effort })}
        onSetMode={(mode: AgentMode) => patchConversation({ mode })}
        editTarget={editTarget}
        onCancelEdit={handleCancelEdit}
        onSend={handleSend}
        onSendEdit={handleSendEdit}
        onStop={() => activeId && stopGeneration(activeId)}
        onOpenLink={openWebPreview}
        onOpenFile={openFilePreview}
        onOpenArtifact={openMediaPreview}
        onEditMessage={handleEditMessage}
      />
      <WebPreviewPanel
        open={previewOpen}
        preview={preview}
        loading={previewLoading}
        error={previewError}
        onClose={() => setPreviewOpen(false)}
        onOpenExternal={(url) => openUrl(url)}
      />
      <SettingsPanel
        open={settingsOpen}
        settings={settings}
        onClose={() => setSettingsOpen(false)}
        onSave={handleSaveSettings}
        onClearAll={handleClearAll}
      />
      {permRequest && (
        <div className="perm-overlay">
          <div className="perm-dialog">
            <div className="perm-dialog-title">访问权限确认</div>
            <div className="perm-dialog-body">
              <p>AI 助手请求访问<b>会话目录之外</b>的文件：</p>
              <div className="perm-path">
                {permRequest.path}
              </div>
              <div className="perm-tool">
                操作：{permRequest.tool}（默认不允许访问会话目录外的文件夹）
              </div>
            </div>
            <div className="perm-dialog-actions">
              <button
                className="btn-danger"
                onClick={() => {
                  respondPermission(permRequest.conversation_id, false).catch(() => {});
                  setPermRequest(null);
                }}
              >
                拒绝
              </button>
              <button
                className="btn-primary"
                onClick={() => {
                  respondPermission(permRequest.conversation_id, true).catch(() => {});
                  setPermRequest(null);
                }}
              >
                允许本次访问
              </button>
            </div>
          </div>
        </div>
      )}
      {errorBanner && (
        <div className="error-banner" onClick={dismissError}>
          <span>{errorBanner}</span>
          <button className="error-banner-close" title="关闭">
            <XIcon size={12} />
          </button>
        </div>
      )}
      <ToastStack toasts={toasts} onDismiss={dismissToast} />
    </div>
  );
}
