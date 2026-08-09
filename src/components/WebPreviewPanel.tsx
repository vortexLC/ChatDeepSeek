import type { WebPage } from "../types";
import { ExternalIcon, LinkIcon, XIcon } from "./icons";

interface WebPreviewPanelProps {
  open: boolean;
  preview: WebPage | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
  onOpenExternal: (url: string) => void;
}

export function WebPreviewPanel({
  open,
  preview,
  loading,
  error,
  onClose,
  onOpenExternal,
}: WebPreviewPanelProps) {
  if (!open) return null;
  return (
    <aside className="web-panel">
      <div className="web-panel-header">
        <div className="web-panel-title-wrap">
          <div className="web-panel-title" title={preview?.title ?? "网页预览"}>
            {preview?.title || "网页预览"}
          </div>
          {preview && <div className="web-panel-url">{preview.url}</div>}
        </div>
        {preview && (
          <button
            className="web-panel-icon-btn"
            onClick={() => onOpenExternal(preview.url)}
            title="在浏览器中打开"
          >
            <ExternalIcon size={14} />
          </button>
        )}
        <button
          className="web-panel-icon-btn"
          onClick={onClose}
          title="关闭面板"
        >
          <XIcon size={15} />
        </button>
      </div>
      <div className="web-panel-body">
        {loading && (
          <div className="web-panel-loading">
            <LinkIcon size={14} />
            正在加载网页内容…
          </div>
        )}
        {error && <div className="web-panel-error">{error}</div>}
        {!loading && !error && preview && preview.html && (
          <iframe
            className="web-panel-frame"
            sandbox=""
            srcDoc={preview.html}
            title={preview.url}
          />
        )}
        {!loading && !error && !preview && (
          <div className="web-panel-empty">
            点击搜索结果或回答中的来源链接，即可在右侧预览网页内容
          </div>
        )}
      </div>
    </aside>
  );
}
