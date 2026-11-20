import "./styles/global.css";
import "./lib/i18n";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { installGlobalErrorHandlers } from "./lib/report";
import { applyBackdrop, applyTheme, resolveTheme, systemPrefersDark } from "./lib/theme";

// 渲染前先按系统深浅色设好主题，避免第一帧闪色
applyTheme(resolveTheme("system", systemPrefersDark()));
applyBackdrop("solid");
installGlobalErrorHandlers();

const container = document.getElementById("root");
if (!container) throw new Error("页面缺少 #root 容器");

createRoot(container).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
