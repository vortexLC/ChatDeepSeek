import { useEffect, useState } from "react";
import type { Job } from "../types";
import { AlertIcon, VideoIcon } from "./icons";

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes()
  ).padStart(2, "0")}`;
}

function fmtWait(ms: number): string {
  const secs = Math.max(0, Math.floor(ms / 1000));
  return `${String(Math.floor(secs / 60)).padStart(2, "0")}:${String(
    secs % 60
  ).padStart(2, "0")}`;
}

/** 单个异步任务卡片：生成中（实时计时）/ 失败（错误信息） */
function JobCard({ job }: { job: Job }) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (job.status !== "pending") return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [job.status]);

  if (job.status === "pending") {
    return (
      <div className="job-card pending">
        <span className="job-card-icon">
          <span className="dot-pulse" />
          <VideoIcon size={15} />
        </span>
        <div className="job-card-main">
          <div className="job-card-title">视频生成中</div>
          <div className="job-card-sub">
            {job.model} · 提交于 {formatTime(job.submitted_at)} · 已等待{" "}
            {fmtWait(now - job.submitted_at)}
          </div>
        </div>
      </div>
    );
  }
  if (job.status === "failed") {
    return (
      <div className="job-card failed">
        <span className="job-card-icon">
          <AlertIcon size={16} />
        </span>
        <div className="job-card-main">
          <div className="job-card-title">视频生成失败</div>
          <div className="job-card-sub">{job.error || "未知错误"}</div>
        </div>
      </div>
    );
  }
  return null;
}

/** 会话底部任务卡片列表：仅展示进行中 / 失败的任务（已完成由总结消息承载） */
export function JobCards({ jobs }: { jobs: Job[] }) {
  const visible = jobs.filter(
    (j) => j.status === "pending" || j.status === "failed"
  );
  if (visible.length === 0) return null;
  return (
    <div className="job-cards">
      {visible.map((j) => (
        <JobCard key={j.id} job={j} />
      ))}
    </div>
  );
}
