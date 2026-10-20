import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import MeetingIndicator from "./MeetingIndicator";
import {
  applyTheme,
  getStoredTheme,
  syncThemeFromSettings,
} from "@/lib/utils/theme";
import type { Theme } from "@/bindings";
import "@/i18n";
import "./MeetingIndicator.css";

// Its own webview, like the dictation overlay: follow the app theme.
applyTheme(getStoredTheme());
syncThemeFromSettings();
listen<Theme>("theme-changed", (event) => applyTheme(event.payload));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <MeetingIndicator />
  </React.StrictMode>,
);
