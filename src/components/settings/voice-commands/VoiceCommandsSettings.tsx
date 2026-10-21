import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import type { AppSettings } from "@/bindings";
import { Alert } from "../../ui/Alert";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { ShortcutInput } from "../ShortcutInput";
import { PostProcessingSettingsApi } from "../PostProcessingSettingsApi";
import { useSettings } from "../../../hooks/useSettings";
import { useOsType } from "../../../hooks/useOsType";
import { formatKeyCombination } from "../../../lib/utils/keyboard";

const BINDING_ID = "voice_command";
const APPLE_PROVIDER_ID = "apple_intelligence";

/** Mirrors `llm_config_ready` in voice_command.rs. */
const isLlmConfigured = (
  settings: AppSettings | null,
  appleAvailable: boolean,
): boolean => {
  if (!settings) return false;
  const providerId = settings.post_process_provider_id;
  const provider = settings.post_process_providers?.find(
    (p) => p.id === providerId,
  );
  if (!provider) return false;
  if (provider.id === APPLE_PROVIDER_ID) return appleAvailable;
  const model = settings.post_process_models?.[provider.id]?.trim() ?? "";
  const key = settings.post_process_api_keys?.[provider.id]?.trim() ?? "";
  return model.length > 0 && (provider.id === "custom" || key.length > 0);
};

const EXAMPLE_KEYS = ["list", "translate", "polite", "summarize", "email"];

const Step: React.FC<{ index: number; children: React.ReactNode }> = ({
  index,
  children,
}) => (
  <li className="flex items-start gap-3">
    <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full border border-mid-gray/30 text-[11px] font-medium text-mid-gray tabular-nums">
      {index}
    </span>
    <span className="text-sm text-text/80 leading-5">{children}</span>
  </li>
);

export const VoiceCommandsSettings: React.FC = () => {
  const { t } = useTranslation();
  const { settings, getSetting, updateSetting, isUpdating } = useSettings();
  const osType = useOsType();
  const [appleAvailable, setAppleAvailable] = useState(false);

  const enabled = getSetting("voice_commands_enabled") ?? false;
  const bindings = getSetting("bindings");
  const shortcut = formatKeyCombination(
    bindings?.[BINDING_ID]?.current_binding ?? "",
    osType,
  );
  const providerId = getSetting("post_process_provider_id");

  useEffect(() => {
    if (providerId !== APPLE_PROVIDER_ID) return;
    let cancelled = false;
    commands
      .checkAppleIntelligenceAvailable()
      .then((available) => {
        if (!cancelled) setAppleAvailable(available);
      })
      .catch(() => {
        if (!cancelled) setAppleAvailable(false);
      });
    return () => {
      cancelled = true;
    };
  }, [providerId]);

  const llmReady = isLlmConfigured(settings, appleAvailable);

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.voiceCommands.title")}>
        <ToggleSwitch
          checked={enabled}
          onChange={(value) => updateSetting("voice_commands_enabled", value)}
          isUpdating={isUpdating("voice_commands_enabled")}
          label={t("settings.voiceCommands.enable.label")}
          description={t("settings.voiceCommands.enable.description")}
          descriptionMode="tooltip"
          grouped={true}
        />
        {enabled && (
          <ShortcutInput
            shortcutId={BINDING_ID}
            descriptionMode="tooltip"
            grouped={true}
          />
        )}
      </SettingsGroup>

      {enabled && (
        <SettingsGroup
          title={t("settings.voiceCommands.model.title")}
          description={t("settings.voiceCommands.model.description")}
        >
          {!llmReady && (
            <Alert variant="warning" contained>
              {t("settings.voiceCommands.model.missing")}
            </Alert>
          )}
          <PostProcessingSettingsApi />
        </SettingsGroup>
      )}

      <SettingsGroup title={t("settings.voiceCommands.howTo.title")}>
        <div className="px-4 py-3 space-y-4">
          <ol className="space-y-2.5">
            <Step index={1}>{t("settings.voiceCommands.howTo.select")}</Step>
            <Step index={2}>
              {t("settings.voiceCommands.howTo.hold", {
                shortcut: shortcut || "…",
              })}
            </Step>
            <Step index={3}>{t("settings.voiceCommands.howTo.release")}</Step>
          </ol>
          <div className="space-y-2">
            <p className="text-xs font-medium text-mid-gray uppercase tracking-wide">
              {t("settings.voiceCommands.examples.title")}
            </p>
            <div className="flex flex-wrap gap-1.5">
              {EXAMPLE_KEYS.map((key) => (
                <span
                  key={key}
                  className="rounded-full border border-mid-gray/20 px-2.5 py-1 text-xs text-text/75"
                >
                  {t(`settings.voiceCommands.examples.${key}`)}
                </span>
              ))}
            </div>
          </div>
          <p className="text-xs text-mid-gray/80">
            {t("settings.voiceCommands.howTo.privacy")}
          </p>
        </div>
      </SettingsGroup>
    </div>
  );
};
