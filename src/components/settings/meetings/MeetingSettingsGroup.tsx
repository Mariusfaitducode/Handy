import React from "react";
import { useTranslation } from "react-i18next";
import { ask, open } from "@tauri-apps/plugin-dialog";
import type {
  MeetingCapabilities,
  MeetingRetention,
  MeetingSettings,
} from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { Button } from "../../ui/Button";
import { Dropdown } from "../../ui/Dropdown";
import { SettingContainer } from "../../ui/SettingContainer";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { ShortcutInput } from "../ShortcutInput";

const DEFAULTS: MeetingSettings = {
  auto_summary: true,
  export_dir: null,
  auto_export: false,
  retention: "forever",
  show_indicator: true,
};

const RETENTIONS: MeetingRetention[] = [
  "forever",
  "year",
  "days_90",
  "days_30",
];
const RETENTION_DAYS: Record<MeetingRetention, number> = {
  forever: Infinity,
  year: 365,
  days_90: 90,
  days_30: 30,
};

/** Asks for the export folder; `null` if the user cancels. */
export const pickExportFolder = async (
  title: string,
  current: string | null,
): Promise<string | null> => {
  const picked = await open({
    directory: true,
    multiple: false,
    title,
    defaultPath: current ?? undefined,
  });
  return typeof picked === "string" ? picked : null;
};

export const MeetingSettingsGroup: React.FC<{
  capabilities: MeetingCapabilities | null;
}> = ({ capabilities }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const settings = getSetting("meeting") ?? DEFAULTS;
  const updating = isUpdating("meeting");

  const update = (patch: Partial<MeetingSettings>) =>
    updateSetting("meeting", { ...settings, ...patch });

  const model = capabilities?.summary_model ?? null;
  const summaryDescription = model
    ? `${t("meetings.settings.autoSummary.description")} ${t(
        "meetings.settings.autoSummary.using",
        { model },
      )}`
    : `${t("meetings.settings.autoSummary.description")} ${t(
        "meetings.settings.autoSummary.notConfigured",
      )}`;

  // A shorter retention deletes meetings right away: confirm first.
  const changeRetention = async (retention: MeetingRetention) => {
    if (RETENTION_DAYS[retention] < RETENTION_DAYS[settings.retention]) {
      const confirmed = await ask(
        t("meetings.settings.retention.confirmBody", {
          period: t(`meetings.settings.retention.${retention}`),
        }),
        {
          title: t("meetings.settings.retention.confirmTitle"),
          kind: "warning",
        },
      );
      if (!confirmed) return;
    }
    update({ retention });
  };

  const chooseFolder = async () => {
    const dir = await pickExportFolder(
      t("meetings.detail.chooseFolder"),
      settings.export_dir,
    );
    if (dir) update({ export_dir: dir });
  };

  return (
    <SettingsGroup title={t("meetings.settings.title")}>
      <ShortcutInput shortcutId="toggle_meeting" grouped={true} />
      <ToggleSwitch
        checked={settings.auto_summary}
        onChange={(auto_summary) => update({ auto_summary })}
        isUpdating={updating}
        label={t("meetings.settings.autoSummary.label")}
        description={summaryDescription}
        descriptionMode="tooltip"
        grouped={true}
      />
      <SettingContainer
        title={t("meetings.settings.exportDir.label")}
        description={t("meetings.settings.exportDir.description")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <div className="flex items-center gap-2 max-w-64">
          <span
            className="text-xs text-mid-gray truncate"
            title={settings.export_dir ?? undefined}
            dir="ltr"
          >
            {settings.export_dir
              ? settings.export_dir.replace(/^\/Users\/[^/]+/, "~")
              : t("meetings.settings.exportDir.none")}
          </span>
          <Button variant="secondary" size="sm" onClick={chooseFolder}>
            {t("meetings.settings.exportDir.choose")}
          </Button>
        </div>
      </SettingContainer>
      <ToggleSwitch
        checked={settings.auto_export}
        onChange={(auto_export) => update({ auto_export })}
        disabled={!settings.export_dir}
        isUpdating={updating}
        label={t("meetings.settings.autoExport.label")}
        description={t("meetings.settings.autoExport.description")}
        descriptionMode="tooltip"
        grouped={true}
      />
      <SettingContainer
        title={t("meetings.settings.retention.label")}
        description={t("meetings.settings.retention.description")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <Dropdown
          options={RETENTIONS.map((value) => ({
            value,
            label: t(`meetings.settings.retention.${value}`),
          }))}
          selectedValue={settings.retention}
          onSelect={(value) => changeRetention(value as MeetingRetention)}
          disabled={updating}
        />
      </SettingContainer>
      <ToggleSwitch
        checked={settings.show_indicator}
        onChange={(show_indicator) => update({ show_indicator })}
        isUpdating={updating}
        label={t("meetings.settings.indicator.label")}
        description={t("meetings.settings.indicator.description")}
        descriptionMode="tooltip"
        grouped={true}
        tooltipPosition="bottom"
      />
    </SettingsGroup>
  );
};
