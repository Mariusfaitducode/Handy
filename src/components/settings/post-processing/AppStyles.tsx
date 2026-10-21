import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { type } from "@tauri-apps/plugin-os";
import { X } from "lucide-react";
import {
  commands,
  type AppStyleRow,
  type LLMPrompt,
  type RunningApp,
} from "@/bindings";

import { Dropdown, SettingsGroup, ToggleSwitch } from "@/components/ui";
import { useSettings } from "../../../hooks/useSettings";

// Dropdown value standing for "no app style: use the selected prompt".
const DEFAULT_VALUE = "__default__";

interface AppStyleRowItemProps {
  row: AppStyleRow;
  prompts: LLMPrompt[];
  defaultLabel: string;
  onChange: (row: AppStyleRow, promptId: string | null) => void;
  onForget: (row: AppStyleRow) => void;
}

const AppStyleRowItem: React.FC<AppStyleRowItemProps> = ({
  row,
  prompts,
  defaultLabel,
  onChange,
  onForget,
}) => {
  const { t } = useTranslation();

  const options = useMemo(
    () => [
      { value: DEFAULT_VALUE, label: defaultLabel },
      ...prompts.map((p) => ({
        value: p.id,
        label: p.name,
        description:
          p.id === row.suggested_prompt_id
            ? t("settings.postProcessing.appStyles.suggested")
            : undefined,
      })),
    ],
    [prompts, defaultLabel, row.suggested_prompt_id, t],
  );

  return (
    <div className="group flex items-center justify-between gap-3 px-4 py-2">
      <div className="min-w-0">
        <p className="text-sm font-medium truncate" title={row.app_name}>
          {row.app_name}
        </p>
        {row.is_web && (
          <p className="text-xs text-mid-gray">
            {t("settings.postProcessing.appStyles.website")}
          </p>
        )}
      </div>
      <div className="flex items-center gap-1 shrink-0">
        <Dropdown
          options={options}
          selectedValue={row.prompt_id ?? DEFAULT_VALUE}
          onSelect={(value) =>
            onChange(row, value === DEFAULT_VALUE ? null : value)
          }
          className="w-56"
        />
        <button
          type="button"
          onClick={() => onForget(row)}
          aria-label={t("settings.postProcessing.appStyles.forget", {
            name: row.app_name,
          })}
          title={t("settings.postProcessing.appStyles.forget", {
            name: row.app_name,
          })}
          className="p-1 rounded text-mid-gray opacity-0 group-hover:opacity-100 focus:opacity-100 hover:text-text hover:bg-mid-gray/20 transition-opacity"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      </div>
    </div>
  );
};

const AppStylesComponent: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [rows, setRows] = useState<AppStyleRow[]>([]);
  const [runningApps, setRunningApps] = useState<RunningApp[]>([]);

  const enabled = getSetting("app_styles_enabled") ?? true;
  const prompts = getSetting("post_process_prompts") || [];
  const selectedPromptId = getSetting("post_process_selected_prompt_id");
  const defaultPrompt = prompts.find((p) => p.id === selectedPromptId);

  const refreshRows = useCallback(async () => {
    try {
      setRows(await commands.getAppStyles());
    } catch (error) {
      console.error("Failed to load app styles:", error);
    }
  }, []);

  const refreshRunningApps = useCallback(async () => {
    try {
      setRunningApps(await commands.listRunningAppsForStyles());
    } catch (error) {
      console.error("Failed to list running apps:", error);
    }
  }, []);

  // Rows depend on the prompt list (names, deletions), so refetch with it.
  useEffect(() => {
    refreshRows();
  }, [refreshRows, prompts]);

  // A dictation can add an app while this page is open.
  useEffect(() => {
    const unlisten = listen("app-styles-changed", () => refreshRows());
    const onFocus = () => refreshRows();
    window.addEventListener("focus", onFocus);
    return () => {
      unlisten.then((fn) => fn());
      window.removeEventListener("focus", onFocus);
    };
  }, [refreshRows]);

  useEffect(() => {
    refreshRunningApps();
  }, [refreshRunningApps]);

  const handleChange = async (row: AppStyleRow, promptId: string | null) => {
    // Optimistic: the dropdown should not snap back while saving.
    setRows((current) =>
      current.map((r) =>
        r.app_key === row.app_key ? { ...r, prompt_id: promptId } : r,
      ),
    );
    try {
      await commands.setAppStyle(row.app_key, row.app_name, promptId);
    } catch (error) {
      console.error("Failed to set app style:", error);
    }
    refreshRows();
  };

  const handleForget = async (row: AppStyleRow) => {
    setRows((current) => current.filter((r) => r.app_key !== row.app_key));
    try {
      await commands.forgetAppStyleApp(row.app_key);
    } catch (error) {
      console.error("Failed to remove app:", error);
    }
    refreshRows();
  };

  const handleAdd = async (appKey: string) => {
    const app = runningApps.find((a) => a.app_key === appKey);
    if (!app) return;
    try {
      await commands.addAppStyleApp(app.app_key, app.app_name);
    } catch (error) {
      console.error("Failed to add app:", error);
    }
    refreshRows();
  };

  const defaultLabel = defaultPrompt
    ? t("settings.postProcessing.appStyles.defaultOption", {
        name: defaultPrompt.name,
      })
    : t("settings.postProcessing.appStyles.defaultOptionNone");

  const addOptions = runningApps
    .filter((a) => !rows.some((r) => r.app_key === a.app_key))
    .map((a) => ({ value: a.app_key, label: a.app_name }));

  return (
    <>
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("app_styles_enabled", value)}
        isUpdating={isUpdating("app_styles_enabled")}
        label={t("settings.postProcessing.appStyles.enabled.label")}
        description={t("settings.postProcessing.appStyles.enabled.description")}
        descriptionMode="inline"
        grouped={true}
      />

      {enabled && (
        <>
          {rows.length === 0 ? (
            <div className="px-4 py-3">
              <p className="text-sm text-mid-gray">
                {t("settings.postProcessing.appStyles.empty")}
              </p>
            </div>
          ) : (
            rows.map((row) => (
              <AppStyleRowItem
                key={row.app_key}
                row={row}
                prompts={prompts}
                defaultLabel={defaultLabel}
                onChange={handleChange}
                onForget={handleForget}
              />
            ))
          )}

          <div className="flex items-center justify-between gap-3 px-4 py-2">
            <p className="text-xs text-mid-gray/80">
              {t("settings.postProcessing.appStyles.hint")}
            </p>
            <Dropdown
              options={addOptions}
              selectedValue={null}
              onSelect={handleAdd}
              onOpen={refreshRunningApps}
              placeholder={t("settings.postProcessing.appStyles.addApp")}
              className="w-56 shrink-0 me-[26px]"
              menuClassName="left-0 right-0"
            />
          </div>
        </>
      )}
    </>
  );
};

export const AppStyles = React.memo(AppStylesComponent);
AppStyles.displayName = "AppStyles";

/** "Style by app" settings group. macOS only: app detection is macOS-only. */
export const AppStylesSettingsGroup: React.FC = () => {
  const { t } = useTranslation();
  if (type() !== "macos") return null;

  return (
    <SettingsGroup
      title={t("settings.postProcessing.appStyles.title")}
      description={t("settings.postProcessing.appStyles.description")}
    >
      <AppStyles />
    </SettingsGroup>
  );
};
