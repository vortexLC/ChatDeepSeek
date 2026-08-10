import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import type { AppSettings } from "../types";
import { testDeepSeekConnection } from "../api";

const TABS = [
  { id: "general", label: "通用" },
  { id: "models", label: "AI 模型" },
  { id: "search", label: "搜索服务" },
  { id: "gen", label: "图像视频" },
] as const;

type TabId = (typeof TABS)[number]["id"];

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
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; msg: string } | null>(
    null
  );

  useEffect(() => {
    if (open) {
      setDraft(settings);
      setTab("general");
      setConfirmClear(false);
      setTestResult(null);
    }
  }, [open, settings]);

  const runTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const msg = await testDeepSeekConnection(draft.deepseek.api_key.trim());
      setTestResult({ ok: true, msg });
    } catch (e) {
      setTestResult({ ok: false, msg: String(e) });
    } finally {
      setTesting(false);
    }
  };

  if (!open) return null;

  const set = <K extends keyof AppSettings>(key: K, value: AppSettings[K]) =>
    setDraft((d) => ({ ...d, [key]: value }));
  const setDeepSeek = (key: keyof AppSettings["deepseek"], value: string) =>
    setDraft((d) => ({ ...d, deepseek: { ...d.deepseek, [key]: value } }));
  const setGen = <K extends keyof AppSettings["gen"]>(
    key: K,
    value: AppSettings["gen"][K]
  ) => setDraft((d) => ({ ...d, gen: { ...d.gen, [key]: value } }));
  const setSF = (key: keyof AppSettings["gen"]["siliconflow"], value: string) =>
    setDraft((d) => ({
      ...d,
      gen: { ...d.gen, siliconflow: { ...d.gen.siliconflow, [key]: value } },
    }));
  const setSearch = <K extends keyof AppSettings["search"]>(
    key: K,
    value: AppSettings["search"][K]
  ) => setDraft((d) => ({ ...d, search: { ...d.search, [key]: value } }));

  return (
    <div className="settings-overlay" onClick={onClose}>
      <div
        className="settings-panel"
        onClick={(e) => e.stopPropagation()}
      >
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
              <Field label="默认模型">
                <select
                  value={draft.default_model}
                  onChange={(e) => set("default_model", e.target.value)}
                >
                  <option value="deepseek-v4-flash">DeepSeek V4 Flash</option>
                  <option value="deepseek-v4-pro">DeepSeek V4 Pro</option>
                </select>
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
                  onChange={(e) =>
                    set("theme", e.target.value as AppSettings["theme"])
                  }
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

          {tab === "models" && (
            <>
              <div className="settings-section-title">DeepSeek API</div>
              <Field
                label="DeepSeek API Key"
                hint="从 platform.deepseek.com 获取。一个 Key 即可同时使用 deepseek-v4-flash 与 deepseek-v4-pro 两个模型，应用通过 Anthropic (Messages API) 协议调用。"
              >
                <div className="field-with-toggle">
                  <input
                    type="password"
                    value={draft.deepseek.api_key}
                    placeholder="sk-..."
                    onChange={(e) => setDeepSeek("api_key", e.target.value)}
                  />
                  <button
                    className="btn-plain"
                    onClick={runTest}
                    disabled={testing || !draft.deepseek.api_key.trim()}
                  >
                    {testing ? "测试中…" : "测试连接"}
                  </button>
                </div>
                {testResult && (
                  <div className={`test-result${testResult.ok ? " ok" : " fail"}`}>
                    {testResult.msg}
                  </div>
                )}
              </Field>
              <Field label="模型选择" hint="发送消息框左侧可选择当前对话使用的模型（DeepSeek V4 Flash / DeepSeek V4 Pro）">
                <div className="field-static">
                  <span className="chip">DeepSeek V4 Flash</span>
                  <span className="chip">DeepSeek V4 Pro</span>
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

          {tab === "gen" && (
            <>
              <div className="settings-section-title">生成服务提供商</div>
              <Field label="提供商" hint="当前适配硅基流动，模块化设计支持后续扩展其它提供商">
                <select
                  value={draft.gen.provider}
                  onChange={(e) => setGen("provider", e.target.value)}
                >
                  <option value="siliconflow">硅基流动 SiliconFlow</option>
                </select>
              </Field>

              <div className="settings-section-title">硅基流动（图片 / 视频生成）</div>
              <Field
                label="API Key"
                hint="从 cloud.siliconflow.cn 获取。用于 Image（图片生成）与 Video（视频生成）模式。"
              >
                <input
                  type="password"
                  value={draft.gen.siliconflow.api_key}
                  placeholder="sk-..."
                  onChange={(e) => setSF("api_key", e.target.value)}
                />
              </Field>
              <Field label="API Base URL">
                <input
                  type="text"
                  value={draft.gen.siliconflow.base_url}
                  onChange={(e) => setSF("base_url", e.target.value)}
                />
              </Field>
              <Field label="图片生成模型" hint="推荐 Kwai-Kolors/Kolors">
                <input
                  type="text"
                  value={draft.gen.siliconflow.image_model}
                  onChange={(e) => setSF("image_model", e.target.value)}
                />
              </Field>
              <div className="field-row">
                <Field label="视频生成模型（文生视频）" hint="Wan2.2-T2V-A14B">
                  <input
                    type="text"
                    value={draft.gen.siliconflow.video_model_t2v}
                    onChange={(e) => setSF("video_model_t2v", e.target.value)}
                  />
                </Field>
                <Field label="视频生成模型（图生视频）" hint="Wan2.2-I2V-A14B">
                  <input
                    type="text"
                    value={draft.gen.siliconflow.video_model_i2v}
                    onChange={(e) => setSF("video_model_i2v", e.target.value)}
                  />
                </Field>
              </div>
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
        </div>
      </div>
      </div>
    </div>
  );
}
