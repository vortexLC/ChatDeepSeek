import { useEffect, useState } from "react";
import type { Artifact } from "../types";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getArtifactAbsPath } from "../api";
import { ImageIcon, LinkIcon, VideoIcon } from "./icons";

// 会话内相对路径 -> 可展示 URL（asset 协议）缓存，避免重复 IPC；
// 采用容量上限（LRU 语义：超限时移除最早插入的条目），防止长期使用内存无限增长
const absSrcCache = new Map<string, string>();
const ABS_SRC_CACHE_MAX = 120;
// 视频路径 -> 首帧 dataURL，避免重复截帧
const videoThumbCache = new Map<string, string>();
const VIDEO_THUMB_CACHE_MAX = 60;

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

/**
 * 截取视频首帧：隐藏 <video> 加载后 seek 到 0.1s，用 canvas 抽帧转 JPEG dataURL。
 * 结果按 会话:路径 缓存，避免同一视频重复解码；失败时回退视频图标。
 */
function VideoThumb({
  convId,
  path,
  name,
}: {
  convId: number;
  path: string;
  name: string;
}) {
  const src = useArtifactSrc(convId, path);
  const [thumb, setThumb] = useState<string | null>(() =>
    videoThumbCache.get(`${convId}:${path}`) ?? null
  );
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!src || thumb || failed) return;
    const video = document.createElement("video");
    video.muted = true;
    video.playsInline = true;
    video.preload = "metadata";
    let settled = false;
    let timer = 0;
    const done = (v: string | null) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      if (v) {
        cacheSet(videoThumbCache, `${convId}:${path}`, v, VIDEO_THUMB_CACHE_MAX);
        setThumb(v);
      } else {
        setFailed(true);
      }
    };
    const capture = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = video.videoWidth || 320;
        canvas.height = video.videoHeight || 180;
        const ctx = canvas.getContext("2d");
        if (!ctx) return done(null);
        ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
        done(canvas.toDataURL("image/jpeg", 0.72));
      } catch {
        done(null);
      }
    };
    video.onloadedmetadata = () => {
      // 部分浏览器需要事件循环后再 seek
      requestAnimationFrame(() => {
        try {
          video.currentTime = Math.min(0.1, (video.duration || 0.5) / 2);
        } catch {
          done(null);
        }
      });
    };
    video.onseeked = capture;
    video.oncanplay = () => {
      // seek 未触发（如时长极短/无关键帧）时的兜底
      if (video.readyState >= 2 && !settled) capture();
    };
    video.onerror = () => done(null);
    // 8 秒兜底超时，避免个别视频卡住组件
    timer = window.setTimeout(() => done(null), 8000);
    video.src = src;
    return () => {
      window.clearTimeout(timer);
      video.removeAttribute("src");
      video.load();
    };
  }, [src, thumb, failed, convId, path]);

  if (thumb) return <img className="artifact-thumb" src={thumb} alt={name} />;
  return (
    <div className={`artifact-thumb placeholder${failed ? " failed" : ""}`}>
      <VideoIcon size={20} />
    </div>
  );
}

/** 产物卡片 v2：图片=缩略图卡片；视频=首帧缩略图+播放角标；文件=名称芯片 */
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
                {a.kind === "image" ? (
                  <ArtifactImage convId={convId} path={a.path} name={a.name} />
                ) : (
                  <VideoThumb convId={convId} path={a.path} name={a.name} />
                )}
                {a.kind === "video" && (
                  <span className="artifact-play-overlay">
                    <span className="artifact-play-btn">
                      <VideoIcon size={14} />
                    </span>
                  </span>
                )}
              </span>
              <span className="artifact-card-footer">
                <span className="artifact-name" title={a.name}>
                  {a.name}
                </span>
                <span className="artifact-badge">
                  {a.kind === "image" ? (
                    <ImageIcon size={11} />
                  ) : (
                    <VideoIcon size={11} />
                  )}
                  {a.kind === "image" ? "图片" : "视频"}
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
