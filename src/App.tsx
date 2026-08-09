import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  clearAllConversations,
  createConversation,
  deleteConversation,
  editAndResend,
  fetchWebPage,
  getContextStatus,
  getInitialState,
  getMessages,
  onChatEvent,
  saveSettings,
  sendMessage,
  stopGeneration,
  updateConversation,
} from "./api";
import type { ConversationUpdate } from "./api";
import type {
  AppSettings,
  ChatDraft,
  ChatEventPayload,
  ChatStatus,
  ContextStatus,
  Conversation,
  EditTarget,
  Effort,
  Message,
  ModelOption,
  WebPage,
} from "./types";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { SettingsPanel } from "./components/SettingsPanel";
import { WebPreviewPanel } from "./components/WebPreviewPanel";
import { XIcon } from "./components/icons";

const DEFAULT_SETTINGS: AppSettings = {
  theme: "auto",
  default_web_search: false,
  default_deep_think: false,
  default_effort: "high",
  default_model: "deepseek-v4-flash",
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
};

const MODEL_OPTIONS: ModelOption[] = [
  { label: "DeepSeek V4 Flash", model: "deepseek-v4-flash", family: "flash" },
  { label: "DeepSeek V4 Pro", model: "deepseek-v4-pro", family: "pro" },
];

function emptyDraft(): ChatDraft {
  return {
    status: "idle",
    reasoning: "",
    content: "",
    searchItems: [],
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
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [errorBanner, setErrorBanner] = useState<string | null>(null);
  const [context, setContext] = useState<ContextStatus | null>(null);
  const [preview, setPreview] = useState<WebPage | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [editTarget, setEditTarget] = useState<EditTarget | null>(null);
  const activeIdRef = useRef<number | null>(null);
  const errorTimerRef = useRef<number | null>(null);
  activeIdRef.current = activeId;

  const showError = useCallback((msg: string) => {
    setErrorBanner(msg);
    if (errorTimerRef.current !== null) {
      window.clearTimeout(errorTimerRef.current);
    }
    errorTimerRef.current = window.setTimeout(() => {
      setErrorBanner(null);
      errorTimerRef.current = null;
    }, 6000);
  }, []);

  const dismissError = useCallback(() => {
    if (errorTimerRef.current !== null) {
      window.clearTimeout(errorTimerRef.current);
      errorTimerRef.current = null;
    }
    setErrorBanner(null);
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
  }, []);

  useEffect(() => {
    if (settings.theme === "auto") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const handler = () => applyTheme("auto");
      mq.addEventListener("change", handler);
      return () => mq.removeEventListener("change", handler);
    }
  }, [settings.theme]);

  useEffect(() => {
    const unlistenPromise = onChatEvent((payload: ChatEventPayload) => {
      handleChatEvent(payload);
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleChatEvent = (payload: ChatEventPayload) => {
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
          if (payload.item) {
            next = { ...cur, searchItems: [...cur.searchItems, payload.item] };
          }
          break;
        case "error":
          next = { ...cur, error: payload.text ?? "生成失败" };
          break;
        case "done":
          return prev;
      }
      return { ...prev, [cid]: next };
    });

    if (payload.kind === "done") {
      setDrafts((prev) => {
        const { [cid]: _removed, ...rest } = prev;
        return rest;
      });
      reloadConversations();
      if (activeIdRef.current === cid) {
        reloadMessages(cid);
        refreshContext(cid);
      }
    }
    if (payload.kind === "error") {
      showError(payload.text ?? "生成失败，请检查 API 配置");
      if (activeIdRef.current === cid) {
        reloadMessages(cid);
        refreshContext(cid);
      }
      setDrafts((prev) => {
        const { [cid]: _removed, ...rest } = prev;
        return rest;
      });
    }
  };

  const reloadConversations = useCallback(async () => {
    try {
      const list = await getInitialState().then((s) => s.conversations);
      setConversations(list);
    } catch {
      /* ignore */
    }
  }, []);

  const reloadMessages = useCallback(async (id: number) => {
    setLoadingMessages(true);
    try {
      setMessages(await getMessages(id));
    } finally {
      setLoadingMessages(false);
    }
  }, []);

  const refreshContext = useCallback(async (id: number) => {
    try {
      setContext(await getContextStatus(id));
    } catch {
      /* ignore */
    }
  }, []);

  const selectConversation = useCallback(
    async (id: number) => {
      setActiveId(id);
      setMessages([]);
      setContext(null);
      setEditTarget(null);
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
      await deleteConversation(id);
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
    [activeId]
  );

  const patchConversation = useCallback(
    (
      patch: ConversationUpdate,
      adjustEffortFor?: ModelOption
    ) => {
      if (!activeId || !activeConversation) return;
      const family = adjustEffortFor
        ? adjustEffortFor.family
        : activeConversation.model.includes("flash")
          ? "flash"
          : "pro";
      let effort = patch.effort ?? activeConversation.effort;
      if (family === "pro" && (effort === "low" || effort === "none")) {
        effort = "high";
      }
      const normalized: ConversationUpdate = { ...patch, effort };
      setConversations((prev) =>
        prev.map((c) => (c.id === activeId ? { ...c, ...normalized } : c))
      );
      updateConversation(activeId, normalized).catch(() => {});
    },
    [activeId, activeConversation]
  );

  const handleSend = useCallback(
    async (content: string) => {
      if (!activeId) return;
      if (context?.full) {
        showError("上下文已满，请新开会话");
        return;
      }
      const tempUser: Message = {
        id: -Date.now(),
        conversation_id: activeId,
        role: "user",
        content,
        reasoning: "",
        search_results: [],
        created_at: Date.now(),
      };
      setMessages((prev) => [...prev, tempUser]);
      setDrafts((prev) => ({ ...prev, [activeId]: emptyDraft() }));
      try {
        await sendMessage(activeId, content);
      } catch (e) {
        showError(`发送失败: ${e}`);
        setMessages((prev) => prev.filter((m) => m.id !== tempUser.id));
        setDrafts((prev) => {
          const { [activeId]: _removed, ...rest } = prev;
          return rest;
        });
      }
    },
    [activeId, context]
  );

  const openWebPreview = useCallback(async (url: string) => {
    setPreviewOpen(true);
    setPreviewError(null);
    setPreviewLoading(true);
    setPreview({ url, title: "", html: "" });
    try {
      const page = await fetchWebPage(url);
      setPreview(page);
    } catch (e) {
      setPreviewError(String(e));
    } finally {
      setPreviewLoading(false);
    }
  }, []);

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
    async (content: string) => {
      if (!activeId || !editTarget) return;
      const tempUser: Message = {
        id: -Date.now(),
        conversation_id: activeId,
        role: "user",
        content,
        reasoning: "",
        search_results: [],
        created_at: Date.now(),
      };
      setEditTarget(null);
      setMessages((prev) => {
        const idx = prev.findIndex((m) => m.id === editTarget.id);
        if (idx < 0) return [...prev, tempUser];
        return [...prev.slice(0, idx), tempUser];
      });
      setDrafts((prev) => ({ ...prev, [activeId]: emptyDraft() }));
      try {
        await editAndResend(activeId, editTarget.id, content);
      } catch (e) {
        showError(`发送失败: ${e}`);
        setMessages((prev) => prev.filter((m) => m.id !== tempUser.id));
        setDrafts((prev) => {
          const { [activeId]: _removed, ...rest } = prev;
          return rest;
        });
      }
    },
    [activeId, editTarget]
  );

  const modelOptions: ModelOption[] = MODEL_OPTIONS;

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
        modelOptions={modelOptions}
        onSelectModel={(option) =>
          patchConversation(
            { provider: "anthropic", model: option.model },
            option
          )
        }
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
        editTarget={editTarget}
        onCancelEdit={handleCancelEdit}
        onSend={handleSend}
        onSendEdit={handleSendEdit}
        onStop={() => activeId && stopGeneration(activeId)}
        onOpenLink={openWebPreview}
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
      {errorBanner && (
        <div className="error-banner" onClick={dismissError}>
          <span>{errorBanner}</span>
          <button className="error-banner-close" title="关闭">
            <XIcon size={12} />
          </button>
        </div>
      )}
    </div>
  );
}
