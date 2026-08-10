import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import type {
  AppSettings,
  ModelConfig,
  ModelSelection,
  ModelType,
  ProviderConfig,
  VideoApi,
} from "../types";
import { testModel } from "../api";
import { ChevronDownIcon, TrashIcon } from "./icons";

const TABS = [
  { id: "general", label: "通用" },
  { id: "providers", label: "服务商" },
  { id: "select", label: "模型选择" },
  { id: "search", label: "搜索服务" },
] as const;

type TabId = (typeof TABS)[number]["id"];

const TYPE_LABEL: Record<ModelType, string> = {
  text: "文本（无视觉）",
  vision: "多模态（视觉）",
  image: "图片生成",
  video: "视频生成",
};

const SELECT_KEY_SEP = "\u0000";

function selKey(sel: ModelSelection | null): string {
  return sel ? `${sel.provider_id}${SELECT_KEY_SEP}${sel.model_id}` : "";
}

function parseSelKey(key: string): ModelSelection | null {
  const idx = key.indexOf(SELECT_KEY_SEP);
  if (idx <= 0) return null;
  return {
    provider_id: key.slice(0, idx),
    model_id: key.slice(idx + 1),
  };
}

// 判断某个“模型选择”槽位引用的模型是否仍然存在且类型匹配
function selectionValid(
  providers: ProviderConfig[],
  sel: ModelSelection | null,
  allowedTypes: ModelType[]
): boolean {
  if (!sel) return false;
  const provider = providers.find((p) => p.id === sel.provider_id);
  const model = provider?.models.find((m) => m.id === sel.model_id);
  if (!model) return false;
  return allowedTypes.includes(model.model_type);
}

// 清理指向已删除模型（或类型已变更模型）的选择，避免悬空引用
function cleanSelections(s: AppSettings, providers: ProviderConfig[]): AppSettings {
  const keep = (
    sel: ModelSelection | null,
    types: ModelType[]
  ) => (sel && selectionValid(providers, sel, types) ? sel : null);
  return {
    ...s,
    chat_model: keep(s.chat_model, ["text", "vision"]),
    image_model: keep(s.image_model, ["image"]),
    video_model_t2v: keep(s.video_model_t2v, ["video"]),
    video_model_i2v: keep(s.video_model_i2v, ["video"]),
    video_model_r2v: keep(s.video_model_r2v, ["video"]),
  };
}

interface SettingsPanelProps {
  open: boolean;
  settings: AppSettings;
  onClose: () => void;
  onSave: (settings: AppSettings) => void;
  onClearAll: () => void;
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="field">
      <div className="field-label">{label}</div>
      {children}
      {hint && <div className="field-hint">{hint}</div>}
    </div>
  );
}

export function SettingsPanel({
  open,
  settings,
  onClose,
  onSave,
  onClearAll,
}: SettingsPanelProps) {
  const [tab, setTab] = useState<TabId>("general");
  const [draft, setDraft] = useState<AppSettings>(settings);
  const [confirmClear, setConfirmClear] = useState(false);
  // 服务商卡片折叠状态（未记录视为展开）
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  // 新增模型表单
  const [addingModelFor, setAddingModelFor] = useState<string | null>(null);
  const [newModelName, setNewModelName] = useState("");
  const [newModelType, setNewModelType] = useState<ModelType>("text");
  const [newModelVideoApi, setNewModelVideoApi] = useState<VideoApi>("auto");
  const [newModelContextK, setNewModelContextK] = useState(128);
  const [modelFormError, setModelFormError] = useState<string | null>(null);
  // 模型测试
  const [testingModel, setTestingModel] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, { ok: boolean; msg: string }>>(
    {}
  );

  useEffect(() => {
    if (open) {
      setDraft(settings);
      setTab("general");
      setConfirmClear(false);
      setCollapsed({});
      setAddingModelFor(null);
      setNewModelName("");
      setNewModelVideoApi("auto");
      setNewModelContextK(128);
      setModelFormError(null);
      setTestResults({});
    }
  }, [open, settings]);

  if (!open) return null;

  const set = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) =>
    setDraft((d) => ({ ...d, [key]: value }));
  const setSearch = <K extends keyof AppSettings["search"]>(
    key: K,
    value: AppSettings["search"][K]
  ) => setDraft((d) => ({ ...d, search: { ...d.search, [key]: value } }));

  // ---------- 服务商操作 ----------
  const updateProvider = (id: string, patch: Partial<ProviderConfig>) =>
    setDraft((d) => ({
      ...d,
      providers: d.providers.map((p) => (p.id === id ? { ...p, ...patch } : p)),
    }));
  const addProvider = () =>
    setDraft((d) => ({
      ...d,
      providers: [
        ...d.providers,
        {
          id: `p_${Date.now()}`,
          name: `服务商 ${d.providers.length + 1}`,
          protocol: "openai",
          api_base: "https://",
          api_key: "",
          models: [],
        },
      ],
    }));
  const removeProvider = (id: string) =>
    setDraft((d) => {
      const providers = d.providers.filter((p) => p.id !== id);
      return cleanSelections({ ...d, providers }, providers);
    });
  const updateModel = (pid: string, mid: string, patch: Partial<ModelConfig>) =>
    setDraft((d) => {
      const providers = d.providers.map((p) =>
        p.id === pid
          ? {
              ...p,
              models: p.models.map((m) =>
                m.id === mid ? { ...m, ...patch } : m
              ),
            }
          : p
      );
      // 类型变更后，清理不再匹配的"模型选择"引用
      return cleanSelections({ ...d, providers }, providers);
    });
  const removeModel = (pid: string, mid: string) =>
    setDraft((d) => {
      const providers = d.providers.map((p) =>
        p.id === pid ? { ...p, models: p.models.filter((m) => m.id !== mid) } : p
      );
      // 同步清理指向该模型的"模型选择"，避免悬空引用
      return cleanSelections({ ...d, providers }, providers);
    });
  const confirmAddModel = (pid: string) => {
    const name = newModelName.trim();
    if (!name) {
      setModelFormError("请输入模型名称");
      return;
    }
    const provider = draft.providers.find((p) => p.id === pid);
    if (provider?.models.some((m) => m.name.trim() === name)) {
      setModelFormError("该提供商下已存在同名模型");
      return;
    }
    const model: ModelConfig = {
      id: `m_${Date.now()}`,
      name,
      model_type: newModelType,
      video_api: newModelType === "video" ? newModelVideoApi : "auto",
      context_tokens:
        newModelType === "text" || newModelType === "vision"
          ? newModelContextK * 1000
          : 128000,
    };
    setDraft((d) => ({
      ...d,
      providers: d.providers.map((p) =>
        p.id === pid ? { ...p, models: [...p.models, model] } : p
      ),
    }));
    setAddingModelFor(null);
    setNewModelName("");
    setModelFormError(null);
  };

  const runModelTest = async (p: ProviderConfig, m: ModelConfig) => {
    setTestingModel(m.id);
    setTestResults((r) => ({ ...r, [m.id]: { ok: true, msg: "测试中…" } }));
    try {
      const msg = await testModel(p, m);
      setTestResults((r) => ({ ...r, [m.id]: { ok: true, msg } }));
    } catch (e) {
      setTestResults((r) => ({ ...r, [m.id]: { ok: false, msg: String(e) } }));
    } finally {
      setTestingModel(null);
    }
  };

  // ---------- 模型选择 ----------
  const chatModels: { p: ProviderConfig; m: ModelConfig }[] = [];
  const imageModels: { p: ProviderConfig; m: ModelConfig }[] = [];
  const videoModels: { p: ProviderConfig; m: ModelConfig }[] = [];
  for (const p of draft.providers) {
    for (const m of p.models) {
      if (m.model_type === "text" || m.model_type === "vision") {
        chatModels.push({ p, m });
      } else if (m.model_type === "image") {
        imageModels.push({ p, m });
      } else if (m.model_type === "video") {
        videoModels.push({ p, m });
      }
    }
  }

  const selectionField = (
    label: string,
    hint: string,
    options: { p: ProviderConfig; m: ModelConfig }[],
    current: ModelSelection | null,
    onSelect: (sel: ModelSelection | null) => void
  ) => (
    <Field label={label} hint={hint}>
      {options.length === 0 ? (
        <div className="field-empty">暂无可用模型，请先到「服务商」页添加</div>
      ) : (
        <select
          value={selKey(current)}
          onChange={(e) => onSelect(parseSelKey(e.target.value))}
        >
          <option value="">（未选择）</option>
          {options.map(({ p, m }) => (
            <option key={`${p.id}${SELECT_KEY_SEP}${m.id}`} value={`${p.id}${SELECT_KEY_SEP}${m.id}`}>
              {p.name} / {m.name}
              {m.model_type === "vision" ? "（多模态）" : ""}
            </option>
          ))}
        </select>
      )}
    </Field>
  );

  return (
    <div className="settings-overlay" onClick={onClose}>
      <div className="settings-panel" onClick={(e) => e.stopPropagation()}>
        <div className="settings-header">
          <div className="settings-header-title">设置</div>
          <div className="settings-header-actions">
            <button className="btn-plain" onClick={onClose}>
              取消
            </button>
            <button
              className="btn-primary"
              onClick={() => {
                onSave(draft);
                onClose();
              }}
            >
              保存
            </button>
          </div>
        </div>

        <div className="settings-body">
          <div className="settings-tabs">
            {TABS.map((t) => (
              <button
                key={t.id}
                className={`settings-tab${tab === t.id ? " active" : ""}`}
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </button>
            ))}
          </div>

          <div className="settings-content">
            {tab === "general" && (
              <>
                <div className="settings-section-title">对话默认</div>
                <Field
                  label="默认对话模型"
                  hint="新对话使用的模型（当前对话可在消息框左侧切换），在「模型选择」页设置"
                >
                  <div className="field-static">
                    <span className="chip">
                      {chatModels.length > 0
                        ? (() => {
                            const cur =
                              draft.chat_model &&
                              chatModels.find(
                                (x) =>
                                  x.p.id === draft.chat_model!.provider_id &&
                                  x.m.id === draft.chat_model!.model_id
                              );
                            return cur
                              ? `${cur.p.name} / ${cur.m.name}`
                              : "已设置的模型已不存在";
                          })()
                        : "暂未添加对话模型"}
                    </span>
                  </div>
                </Field>
                <div className="field-row">
                  <label className="checkbox-field">
                    <input
                      type="checkbox"
                      checked={draft.default_web_search}
                      onChange={(e) => set("default_web_search", e.target.checked)}
                    />
                    新对话默认开启联网搜索
                  </label>
                  <label className="checkbox-field">
                    <input
                      type="checkbox"
                      checked={draft.default_deep_think}
                      onChange={(e) => set("default_deep_think", e.target.checked)}
                    />
                    新对话默认开启深度思考
                  </label>
                </div>
                <Field label="默认推理强度">
                  <select
                    value={draft.default_effort}
                    onChange={(e) =>
                      set("default_effort", e.target.value as AppSettings["default_effort"])
                    }
                  >
                    <option value="low">低</option>
                    <option value="high">高</option>
                    <option value="max">最大</option>
                  </select>
                </Field>
                <Field label="默认模式" hint="新对话默认使用的模式，发送框左侧可随时切换">
                  <select
                    value={draft.default_mode}
                    onChange={(e) =>
                      set("default_mode", e.target.value as AppSettings["default_mode"])
                    }
                  >
                    <option value="chat">Chat（普通对话）</option>
                    <option value="image">Image（+ 图片生成）</option>
                    <option value="video">Video（+ 视频生成）</option>
                    <option value="build">Build（编程工具）</option>
                    <option value="agent">Agent（全部工具）</option>
                  </select>
                </Field>

                <div className="settings-section-title">外观</div>
                <Field label="主题">
                  <select
                    value={draft.theme}
                    onChange={(e) => set("theme", e.target.value as AppSettings["theme"])}
                  >
                    <option value="auto">跟随系统</option>
                    <option value="light">浅色</option>
                    <option value="dark">深色</option>
                  </select>
                </Field>

                <div className="settings-section-title">数据</div>
                <Field label="清空所有对话" hint="将删除全部对话记录，此操作不可恢复">
                  {confirmClear ? (
                    <span className="danger-row">
                      <button
                        className="btn-danger"
                        onClick={() => {
                          onClearAll();
                          setConfirmClear(false);
                        }}
                      >
                        确认清空
                      </button>
                      <button className="btn-plain" onClick={() => setConfirmClear(false)}>
                        取消
                      </button>
                    </span>
                  ) : (
                    <button className="btn-danger" onClick={() => setConfirmClear(true)}>
                      清空所有对话
                    </button>
                  )}
                </Field>
              </>
            )}

            {tab === "providers" && (
              <>
                <div className="settings-section-title">
                  模型服务商
                  <span className="section-hint">可添加多个，名称自定义；协议选择后填写 API 地址与 Key</span>
                </div>
                {draft.providers.map((p) => {
                  const isCollapsed = collapsed[p.id] === true;
                  const keySet = p.api_key.trim().length > 0;
                  return (
                    <div
                      className={`provider-card${isCollapsed ? " collapsed" : ""}`}
                      key={p.id}
                    >
                      <div
                        className="provider-card-head"
                        onClick={() =>
                          setCollapsed((c) => ({ ...c, [p.id]: !isCollapsed }))
                        }
                      >
                        <span
                          className={`provider-key-dot${keySet ? " set" : ""}`}
                          title={keySet ? "已配置 API Key" : "未配置 API Key"}
                        />
                        <span className="provider-card-name">{p.name || "(未命名)"}</span>
                        <span className={`protocol-chip ${p.protocol}`}>
                          {p.protocol === "anthropic" ? "Anthropic" : "OpenAI 兼容"}
                        </span>
                        <span className="provider-models-count">
                          {p.models.length} 个模型
                        </span>
                        <span
                          className={`provider-chevron${isCollapsed ? " collapsed" : ""}`}
                        >
                          <ChevronDownIcon size={14} />
                        </span>
                        <button
                          className="btn-plain btn-sm provider-delete"
                          title="删除服务商"
                          onClick={(e) => {
                            e.stopPropagation();
                            removeProvider(p.id);
                          }}
                        >
                          <TrashIcon size={13} />
                        </button>
                      </div>

                      {!isCollapsed && (
                        <div className="provider-card-body">
                          <div className="field-row">
                            <Field label="名称">
                              <input
                                type="text"
                                value={p.name}
                                placeholder="如：DeepSeek 官方"
                                onChange={(e) =>
                                  updateProvider(p.id, { name: e.target.value })
                                }
                              />
                            </Field>
                            <Field label="API 协议" hint="DeepSeek 官方为 Anthropic；其它大多为 OpenAI 兼容">
                              <select
                                value={p.protocol}
                                onChange={(e) =>
                                  updateProvider(p.id, {
                                    protocol: e.target.value as ProviderConfig["protocol"],
                                  })
                                }
                              >
                                <option value="openai">OpenAI 兼容（chat/completions）</option>
                                <option value="anthropic">Anthropic Messages</option>
                              </select>
                            </Field>
                          </div>
                          <Field label="API Base URL" hint="如 https://api.deepseek.com/anthropic 或 https://api.siliconflow.cn/v1">
                            <input
                              type="text"
                              value={p.api_base}
                              onChange={(e) =>
                                updateProvider(p.id, { api_base: e.target.value })
                              }
                            />
                          </Field>
                          <Field label="API Key">
                            <input
                              type="password"
                              value={p.api_key}
                              placeholder="sk-..."
                              onChange={(e) =>
                                updateProvider(p.id, { api_key: e.target.value })
                              }
                            />
                          </Field>

                          <div className="provider-models">
                            <div className="provider-models-title">已添加模型</div>
                            {p.models.length === 0 && (
                              <div className="field-empty">暂无模型，请点击下方「添加模型」</div>
                            )}
                            {p.models.map((m) => (
                              <div className="model-row" key={m.id}>
                                <div className="model-row-main">
                                  <span className="model-row-name" title={m.name}>
                                    {m.name}
                                  </span>
                                  <span className={`type-chip ${m.model_type}`}>
                                    {TYPE_LABEL[m.model_type] ?? m.model_type}
                                  </span>
                                </div>
                                <div className="model-row-actions">
                                  {(m.model_type === "text" || m.model_type === "vision") && (
                                    <label className="ctx-input" title="上下文容量（千 token）">
                                      上下文
                                      <input
                                        type="number"
                                        min={1}
                                        max={4000}
                                        value={Math.max(1, Math.round((m.context_tokens ?? 128000) / 1000))}
                                        onChange={(e) => {
                                          const k = Math.max(1, Number(e.target.value) || 1);
                                          updateModel(p.id, m.id, {
                                            context_tokens: k * 1000,
                                          });
                                        }}
                                      />
                                      K
                                    </label>
                                  )}
                                  {m.model_type === "video" && (
                                    <select
                                      className="video-api-select"
                                      value={m.video_api}
                                      title="视频生成接口风格"
                                      onChange={(e) =>
                                        updateModel(p.id, m.id, {
                                          video_api: e.target.value as VideoApi,
                                        })
                                      }
                                    >
                                      <option value="auto">接口：自动</option>
                                      <option value="siliconflow">接口：硅基流动</option>
                                      <option value="dashscope">接口：阿里云百炼</option>
                                    </select>
                                  )}
                                  {m.model_type === "text" || m.model_type === "vision" ? (
                                    <span className="type-chip type-select">
                                      <select
                                        value={m.model_type}
                                        title="模型类型（对话模型可选文本或多模态）"
                                        onChange={(e) =>
                                          updateModel(p.id, m.id, {
                                            model_type: e.target.value as ModelType,
                                          })
                                        }
                                      >
                                        <option value="text">文本（无视觉）</option>
                                        <option value="vision">多模态（视觉）</option>
                                      </select>
                                    </span>
                                  ) : (
                                    <span className="type-chip type-select">
                                      <select
                                        value={m.model_type}
                                        title="模型类型"
                                        onChange={(e) =>
                                          updateModel(p.id, m.id, {
                                            model_type: e.target.value as ModelType,
                                          })
                                        }
                                      >
                                        <option value="text">文本（无视觉）</option>
                                        <option value="vision">多模态（视觉）</option>
                                        <option value="image">图片生成</option>
                                        <option value="video">视频生成</option>
                                      </select>
                                    </span>
                                  )}
                                  <button
                                    className="btn-plain btn-sm"
                                    onClick={() => runModelTest(p, m)}
                                    disabled={testingModel === m.id}
                                  >
                                    {testingModel === m.id ? "测试中…" : "测试"}
                                  </button>
                                  <button
                                    className="btn-plain btn-sm"
                                    onClick={() => removeModel(p.id, m.id)}
                                    title="删除模型"
                                  >
                                    <TrashIcon size={12} />
                                  </button>
                                </div>
                              </div>
                            ))}
                            {Object.entries(testResults).map(([mid, r]) => {
                              if (!p.models.some((m) => m.id === mid)) return null;
                              return (
                                <div key={mid} className={`test-result${r.ok ? " ok" : " fail"}`}>
                                  {r.msg}
                                </div>
                              );
                            })}
                            {addingModelFor === p.id ? (
                              <div className="model-add-form">
                                <div className="field-row">
                                  <Field label="模型名称" hint="填写服务商平台上的 Model ID">
                                    <input
                                      type="text"
                                      value={newModelName}
                                      placeholder="如 deepseek-v4-flash"
                                      onChange={(e) => setNewModelName(e.target.value)}
                                    />
                                  </Field>
                                  <Field label="模型类型" hint="测试方式随类型不同（文本发消息、图片生图、视频检查认证）">
                                    <select
                                      value={newModelType}
                                      onChange={(e) =>
                                        setNewModelType(e.target.value as ModelType)
                                      }
                                    >
                                      <option value="text">文本（无视觉）</option>
                                      <option value="vision">多模态（视觉）</option>
                                      <option value="image">图片生成</option>
                                      <option value="video">视频生成</option>
                                    </select>
                                  </Field>
                                </div>
                                {newModelType === "video" ? (
                                  <Field label="视频接口风格" hint="自动：按服务商地址与模型名推断">
                                    <select
                                      value={newModelVideoApi}
                                      onChange={(e) =>
                                        setNewModelVideoApi(e.target.value as VideoApi)
                                      }
                                    >
                                      <option value="auto">自动</option>
                                      <option value="siliconflow">硅基流动（/video/submit）</option>
                                      <option value="dashscope">阿里云百炼（video-synthesis）</option>
                                    </select>
                                  </Field>
                                ) : newModelType === "text" || newModelType === "vision" ? (
                                  <Field label="上下文容量" hint="仅对话/多模态模型需要：上下文窗口大小（千 token），影响上下文自动压缩时机">
                                    <input
                                      type="number"
                                      min={1}
                                      max={4000}
                                      value={newModelContextK}
                                      onChange={(e) =>
                                        setNewModelContextK(
                                          Math.max(1, Number(e.target.value) || 1)
                                        )
                                      }
                                    />
                                  </Field>
                                ) : null}
                                {modelFormError && (
                                  <div className="test-result fail">{modelFormError}</div>
                                )}
                                <div className="danger-row">
                                  <button
                                    className="btn-primary btn-sm"
                                    disabled={!newModelName.trim()}
                                    onClick={() => confirmAddModel(p.id)}
                                  >
                                    添加
                                  </button>
                                  <button
                                    className="btn-plain btn-sm"
                                    onClick={() => {
                                      setAddingModelFor(null);
                                      setNewModelName("");
                                      setModelFormError(null);
                                    }}
                                  >
                                    取消
                                  </button>
                                </div>
                              </div>
                            ) : (
                              <button
                                className="btn-plain btn-sm"
                                onClick={() => {
                                  setAddingModelFor(p.id);
                                  setNewModelName("");
                                  setNewModelType("text");
                                  setNewModelVideoApi("auto");
                                  setNewModelContextK(128);
                                  setModelFormError(null);
                                }}
                              >
                                + 添加模型
                              </button>
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  );
                })}
                <button className="btn-primary" onClick={addProvider}>
                  + 添加服务商
                </button>
                <Field
                  label="说明"
                  hint="对话模型：文本/多模态模型用于聊天；图片生成/视频生成模型用于 Image / Video / Agent 模式中的 AI 生成功能。添加后到「模型选择」页指定用途；对话模型的上下文容量决定上下文进度与自动压缩时机。"
                >
                  <div className="field-static">
                    <span className="chip">文本</span>
                    <span className="chip">多模态</span>
                    <span className="chip">图片生成</span>
                    <span className="chip">视频生成</span>
                  </div>
                </Field>
              </>
            )}

            {tab === "select" && (
              <>
                <div className="settings-section-title">模型使用选择</div>
                {selectionField(
                  "对话模型",
                  "聊天使用；图片输入需选择「多模态」模型（无视觉模型仅支持文本/文档）",
                  chatModels,
                  draft.chat_model,
                  (sel) => set("chat_model", sel)
                )}
                {selectionField(
                  "图片生成模型",
                  "Image / Agent 模式中生成图片使用",
                  imageModels,
                  draft.image_model,
                  (sel) => set("image_model", sel)
                )}
                {selectionField(
                  "文生视频模型",
                  "Video / Agent 模式中文字生成视频使用",
                  videoModels,
                  draft.video_model_t2v,
                  (sel) => set("video_model_t2v", sel)
                )}
                {selectionField(
                  "图生视频模型",
                  "Video / Agent 模式中基于图片（作为首帧）生成视频使用",
                  videoModels,
                  draft.video_model_i2v,
                  (sel) => set("video_model_i2v", sel)
                )}
                {selectionField(
                  "参考生视频模型",
                  "Video / Agent 模式中参考图片 + 提示词生成视频使用（需 r2v 模型，如阿里云百炼 wan2.7-r2v / wan2.6-r2v）",
                  videoModels,
                  draft.video_model_r2v,
                  (sel) => set("video_model_r2v", sel)
                )}
                <Field label="模式说明" hint="Image 模式 = Chat + 图片生成；Video 模式 = Chat + 视频生成；Build = 编程工具（沙箱）；Agent = 全部工具">
                  <div className="field-static">
                    <span className="chip">Chat</span>
                    <span className="chip">Image</span>
                    <span className="chip">Video</span>
                    <span className="chip">Build</span>
                    <span className="chip">Agent</span>
                  </div>
                </Field>
              </>
            )}

            {tab === "search" && (
              <>
                <div className="settings-section-title">搜索引擎</div>
                <Field
                  label="Tavily API Key"
                  hint="适合简单日常任务、事实类数据检索，快速轻量"
                >
                  <div className="field-with-toggle">
                    <input
                      type="password"
                      value={draft.search.tavily_key}
                      placeholder="tvly-..."
                      onChange={(e) => setSearch("tavily_key", e.target.value)}
                    />
                    <label className="checkbox-field">
                      <input
                        type="checkbox"
                        checked={draft.search.tavily_enabled}
                        onChange={(e) => setSearch("tavily_enabled", e.target.checked)}
                      />
                      启用
                    </label>
                  </div>
                </Field>
                <Field
                  label="AnySearch API Key"
                  hint="适合专业垂直领域内容搜索（财经/医疗/学术/代码等），在 anysearch.com/console 创建"
                >
                  <div className="field-with-toggle">
                    <input
                      type="password"
                      value={draft.search.anysearch_key}
                      placeholder="any_..."
                      onChange={(e) => setSearch("anysearch_key", e.target.value)}
                    />
                    <label className="checkbox-field">
                      <input
                        type="checkbox"
                        checked={draft.search.anysearch_enabled}
                        onChange={(e) => setSearch("anysearch_enabled", e.target.checked)}
                      />
                      启用
                    </label>
                  </div>
                </Field>
                <Field label="搜索策略" hint="模型未指定引擎时使用">
                  <select
                    value={draft.search.strategy}
                    onChange={(e) =>
                      setSearch("strategy", e.target.value as AppSettings["search"]["strategy"])
                    }
                  >
                    <option value="auto">智能自动选择</option>
                    <option value="tavily">始终 Tavily</option>
                    <option value="anysearch">始终 AnySearch</option>
                  </select>
                </Field>
                <Field label="每轮搜索结果数" hint="范围 1 - 20，建议 5">
                  <input
                    type="number"
                    min={1}
                    max={20}
                    value={draft.search.max_results}
                    onChange={(e) =>
                      setSearch(
                        "max_results",
                        Math.max(1, Math.min(20, Number(e.target.value) || 5))
                      )
                    }
                  />
                </Field>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
