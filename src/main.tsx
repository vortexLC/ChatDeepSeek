import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
import { attachConsole, error as logError } from "@tauri-apps/plugin-log";

// 日志：前端 console 输出转发到后端统一日志（data/logs/app.log），
// 并捕获前端运行时错误与未处理的 Promise 拒绝，便于定位界面问题
attachConsole();

window.addEventListener("error", (e) => {
  logError(`前端错误: ${e.message} @ ${e.filename}:${e.lineno}:${e.colno}`);
});

window.addEventListener("unhandledrejection", (e) => {
  logError(`未处理的 Promise 拒绝: ${String(e.reason)}`);
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
