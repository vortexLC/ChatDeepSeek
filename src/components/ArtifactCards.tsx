import { useEffect, useState } from "react";
import type { Artifact } from "../types";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getArtifactAbsPath } from "../api";
import { ImageIcon, LinkIcon } from "./icons";

// 会话内相对路径 -> 可展示 URL（asset 协议）缓存，避免重复 IPC；
// 采用容量上限（LRU 语义：超限时移除最早插入的条目），防止长期使用内存无限增长
const absSrcCache = new Map<string, string>();
const ABS_SRC_CACHE_MAX = 120;

function cacheSet<T>(cache: Map<string, T>, key: string, value: T, max: number) {
  if (cache.size >= max) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  cache.set(key, value);
}

/**
 * 解析会话内产物/附件的可展示 URL。
 * 返回 null 表示加载失败（占位态）；undefined 表示加载中。
 */
export function useArtifactSrc(
  convId: number,
  path: string
): string | null | undefined {
  const [src, setSrc] = useState<string | null | undefined>(() =>
    absSrcCache.get(`${convId}:${path}`) ?? null
  );
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (src || failed) return;
    let cancelled = false;
    getArtifactAbsPath(convId, path)
      .then((abs) => {
        if (cancelled) return;
        const url = convertFileSrc(abs);
        cacheSet(absSrcCache, `${convId}:${path}`, url, ABS_SRC_CACHE_MAX);
        setSrc(url);
      })
      .catch(() => {
        if (!cancelled) setFailed(true);
      });
    return () => {
      cancelled = true;
    };
  }, [convId, path, src, failed]);

  if (failed) return null;
  return src;
}

/** 产物卡片：图片=缩略图卡片；文件=名称芯片 */
export function ArtifactCards({
  artifacts,
  onOpenArtifact,
  onOpenFile,
  convId,
}: {
  artifacts: Artifact[];
  onOpenArtifact: (convId: number, artifact: Artifact) => void;
  onOpenFile: (convId: number, path: string, title: string) => void;
  convId: number;
}) {
  if (!artifacts || artifacts.length === 0) return null;
  return (
    <div className="artifact-list">
      {artifacts.map((a, i) => (
        <button
          key={`${a.path}-${i}`}
          className={`artifact-card ${a.kind}`}
          onClick={() =>
            a.kind === "file"
              ? onOpenFile(convId, a.path, a.name)
              : onOpenArtifact(convId, a)
          }
          title={a.path}
        >
          {a.kind === "file" ? (
            <span className="artifact-file">
              <LinkIcon size={13} />
              <span className="artifact-name">{a.name}</span>
              <span className="artifact-note">
                {a.size > 0 ? `${(a.size / 1024).toFixed(0)} KB` : "文件"}
              </span>
            </span>
          ) : (
            <>
              <span className="artifact-media">
                <ArtifactImage convId={convId} path={a.path} name={a.name} />
              </span>
              <span className="artifact-card-footer">
                <span className="artifact-name" title={a.name}>
                  {a.name}
                </span>
                <span className="artifact-badge">
                  <ImageIcon size={11} />
                  图片
                </span>
              </span>
            </>
          )}
        </button>
      ))}
    </div>
  );
}

function ArtifactImage({
  convId,
  path,
  name,
}: {
  convId: number;
  path: string;
  name: string;
}) {
  const src = useArtifactSrc(convId, path);
  if (src === undefined) {
    return (
      <div className="artifact-thumb placeholder">
        <ImageIcon size={20} />
      </div>
    );
  }
  if (src === null) {
    return (
      <div className="artifact-thumb placeholder failed">
        <ImageIcon size={20} />
      </div>
    );
  }
  return <img className="artifact-thumb" src={src} alt={name} />;
}