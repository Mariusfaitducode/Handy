import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ArrowLeft, Check } from "lucide-react";
import {
  commands,
  type MarkdownLayout,
  type VoiceNotesSettings,
  type VoiceNotesStatus,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Alert } from "../../ui/Alert";
import { Button } from "../../ui/Button";
import { Dropdown } from "../../ui/Dropdown";
import { Input } from "../../ui/Input";
import { SettingContainer } from "../../ui/SettingContainer";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { ShortcutInput } from "../ShortcutInput";
import { errorCode, errorKey } from "./noteUtils";

interface NotesSettingsPanelProps {
  onBack: () => void;
}

export const NotesSettingsPanel: React.FC<NotesSettingsPanelProps> = ({
  onBack,
}) => {
  const { t } = useTranslation();
  const { settings, refreshSettings } = useSettings();
  const [status, setStatus] = useState<VoiceNotesStatus | null>(null);
  // Which field is being saved, so only that control shows a spinner.
  const [savingKey, setSavingKey] = useState<string | null>(null);
  const [appleNotesError, setAppleNotesError] = useState<string | null>(null);
  const [checkingAppleNotes, setCheckingAppleNotes] = useState(false);

  const voiceNotes = settings?.voice_notes;

  const loadStatus = useCallback(async () => {
    try {
      setStatus(await commands.getVoiceNotesStatus());
    } catch (error) {
      console.error("Failed to load voice notes status:", error);
    }
  }, []);

  useEffect(() => {
    loadStatus();
  }, [loadStatus]);

  const update = async (patch: Partial<VoiceNotesSettings>) => {
    if (!voiceNotes) return;
    setSavingKey(Object.keys(patch)[0] ?? null);
    try {
      const result = await commands.updateVoiceNotesSettings({
        ...voiceNotes,
        ...patch,
      });
      if (result.status !== "ok") throw new Error(result.error);
    } catch (error) {
      console.error("Failed to save voice notes settings:", error);
      toast.error(t("notes.toasts.saveError"));
    } finally {
      await refreshSettings();
      await loadStatus();
      setSavingKey(null);
    }
  };

  const chooseFolder = async () => {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      defaultPath: status?.effective_folder ?? undefined,
    });
    if (typeof selected === "string" && selected) {
      await update({ markdown_folder: selected });
    }
  };

  const openFolder = async () => {
    const result = await commands.openVoiceNotesFolder();
    if (result.status !== "ok") toast.error(t(errorKey(result.error)));
  };

  const toggleAppleNotes = async (enabled: boolean) => {
    setAppleNotesError(null);
    if (!enabled) {
      await update({ apple_notes_enabled: false });
      return;
    }
    // Ask for Automation access now, while the user is looking, rather than
    // on the first note.
    setCheckingAppleNotes(true);
    try {
      const result = await commands.checkAppleNotesAccess();
      if (result.status === "ok") {
        await update({ apple_notes_enabled: true });
      } else {
        setAppleNotesError(result.error);
      }
    } finally {
      setCheckingAppleNotes(false);
    }
  };

  if (!voiceNotes) return null;

  const layoutOptions = [
    {
      value: "daily",
      label: t("notes.settings.markdown.layout.daily"),
    },
    {
      value: "per_note",
      label: t("notes.settings.markdown.layout.per_note"),
    },
  ];

  const llmProvider = status?.llm_provider ?? null;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="px-4 flex items-center gap-2 min-h-8">
        <button
          type="button"
          onClick={onBack}
          className="p-1 -ms-1 rounded-md text-text/60 hover:text-logo-primary hover:bg-mid-gray/10 transition-colors cursor-pointer"
          title={t("notes.back")}
          aria-label={t("notes.back")}
        >
          <ArrowLeft className="w-4 h-4 rtl:rotate-180" />
        </button>
        <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("notes.settings.title")}
        </h2>
      </div>

      <SettingsGroup title={t("notes.settings.capture.title")}>
        <ToggleSwitch
          label={t("notes.settings.capture.enabled.label")}
          description={t("notes.settings.capture.enabled.description")}
          checked={voiceNotes.enabled}
          onChange={(enabled) => update({ enabled })}
          isUpdating={savingKey === "enabled"}
          grouped
        />
        {voiceNotes.enabled && (
          <>
            <ShortcutInput shortcutId="voice_note" grouped />
            <div>
              <ToggleSwitch
                label={t("notes.settings.capture.ai.label")}
                description={t("notes.settings.capture.ai.description")}
                checked={voiceNotes.ai_cleanup && llmProvider !== null}
                disabled={llmProvider === null}
                onChange={(ai_cleanup) => update({ ai_cleanup })}
                isUpdating={savingKey === "ai_cleanup"}
                grouped
              />
              <p className="px-4 pb-3 -mt-1 text-xs text-mid-gray">
                {llmProvider
                  ? t("notes.settings.capture.ai.using", {
                      provider: llmProvider,
                    })
                  : settings?.post_process_enabled
                    ? t("notes.settings.capture.ai.unavailableNoProvider")
                    : t("notes.settings.capture.ai.unavailableDisabled")}
              </p>
            </div>
          </>
        )}
      </SettingsGroup>

      <SettingsGroup title={t("notes.settings.markdown.title")}>
        <ToggleSwitch
          label={t("notes.settings.markdown.enabled.label")}
          description={t("notes.settings.markdown.enabled.description")}
          checked={voiceNotes.markdown_enabled}
          onChange={(markdown_enabled) => update({ markdown_enabled })}
          isUpdating={savingKey === "markdown_enabled"}
          grouped
        />
        {voiceNotes.markdown_enabled && (
          <>
            <SettingContainer
              title={t("notes.settings.markdown.folder.label")}
              description={t("notes.settings.markdown.folder.description")}
              layout="stacked"
              grouped
            >
              <div className="flex items-center gap-2">
                <div
                  className="flex-1 min-w-0 px-2 py-1.5 bg-mid-gray/10 border border-mid-gray/20 rounded-md text-xs font-mono truncate select-text"
                  title={status?.effective_folder ?? ""}
                >
                  {status?.effective_folder ?? ""}
                </div>
                <Button variant="secondary" size="sm" onClick={chooseFolder}>
                  {t("notes.settings.markdown.folder.choose")}
                </Button>
                <Button variant="secondary" size="sm" onClick={openFolder}>
                  {t("notes.settings.markdown.folder.open")}
                </Button>
                {voiceNotes.markdown_folder && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => update({ markdown_folder: null })}
                  >
                    {t("notes.settings.markdown.folder.reset")}
                  </Button>
                )}
              </div>
            </SettingContainer>
            <SettingContainer
              title={t("notes.settings.markdown.layout.label")}
              description={t("notes.settings.markdown.layout.description")}
              grouped
            >
              <Dropdown
                options={layoutOptions}
                selectedValue={voiceNotes.markdown_layout}
                onSelect={(value) =>
                  update({ markdown_layout: value as MarkdownLayout })
                }
                disabled={savingKey === "markdown_layout"}
              />
            </SettingContainer>
          </>
        )}
      </SettingsGroup>

      <NotionSettings
        voiceNotes={voiceNotes}
        onChanged={async () => {
          await refreshSettings();
        }}
      />

      {status?.apple_notes_supported && (
        <SettingsGroup title={t("notes.settings.appleNotes.title")}>
          <ToggleSwitch
            label={t("notes.settings.appleNotes.label")}
            description={t("notes.settings.appleNotes.description")}
            checked={voiceNotes.apple_notes_enabled}
            onChange={toggleAppleNotes}
            isUpdating={
              savingKey === "apple_notes_enabled" || checkingAppleNotes
            }
            grouped
          />
          {appleNotesError && (
            <div className="px-4 py-3 space-y-2">
              <Alert variant="warning" contained>
                {t(errorKey(appleNotesError))}
              </Alert>
              {errorCode(appleNotesError) === "apple_notes_denied" && (
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() =>
                    commands.openVoiceNotesPrivacySettings("automation")
                  }
                >
                  {t("notes.settings.appleNotes.openSettings")}
                </Button>
              )}
            </div>
          )}
        </SettingsGroup>
      )}
    </div>
  );
};

interface NotionSettingsProps {
  voiceNotes: VoiceNotesSettings;
  onChanged: () => Promise<void>;
}

const NotionSettings: React.FC<NotionSettingsProps> = ({
  voiceNotes,
  onChanged,
}) => {
  const { t } = useTranslation();
  const [token, setToken] = useState("");
  const [page, setPage] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const connect = async () => {
    setConnecting(true);
    setError(null);
    try {
      const result = await commands.connectNotion(token, page);
      if (result.status === "ok") {
        setToken("");
        setPage("");
        await onChanged();
      } else {
        setError(result.error);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setConnecting(false);
    }
  };

  const disconnect = async () => {
    await commands.disconnectNotion();
    await onChanged();
  };

  return (
    <SettingsGroup
      title={t("notes.settings.notion.title")}
      description={t("notes.settings.notion.description")}
    >
      {voiceNotes.notion_connected ? (
        <div className="flex items-center justify-between gap-3 min-h-12 px-4 py-2">
          <p className="flex items-center gap-2 text-sm min-w-0">
            <Check
              className="w-4 h-4 shrink-0 text-logo-primary"
              aria-hidden="true"
            />
            <span className="truncate">
              {voiceNotes.notion_parent_title
                ? t("notes.settings.notion.connected", {
                    page: voiceNotes.notion_parent_title,
                  })
                : t("notes.settings.notion.connectedUntitled")}
            </span>
          </p>
          <Button variant="secondary" size="sm" onClick={disconnect}>
            {t("notes.settings.notion.disconnect")}
          </Button>
        </div>
      ) : (
        <form
          className="px-4 py-3 space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (token.trim() && page.trim() && !connecting) connect();
          }}
        >
          <label className="block space-y-1">
            <span className="text-sm font-medium">
              {t("notes.settings.notion.token.label")}
            </span>
            <Input
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder={t("notes.settings.notion.token.placeholder")}
              className="w-full font-mono"
              variant="compact"
            />
            <span className="block text-xs text-mid-gray leading-relaxed">
              {t("notes.settings.notion.token.help")}
            </span>
          </label>
          <label className="block space-y-1">
            <span className="text-sm font-medium">
              {t("notes.settings.notion.page.label")}
            </span>
            <Input
              type="url"
              spellCheck={false}
              value={page}
              onChange={(e) => setPage(e.target.value)}
              placeholder={t("notes.settings.notion.page.placeholder")}
              className="w-full"
              variant="compact"
            />
          </label>
          {error && (
            <Alert variant="error" contained>
              {t(errorKey(error))}
            </Alert>
          )}
          <div className="flex items-center justify-between gap-3">
            <span className="text-xs text-mid-gray">
              {t("notes.settings.notion.keychain")}
            </span>
            <Button
              type="submit"
              variant="primary-soft"
              size="sm"
              disabled={!token.trim() || !page.trim() || connecting}
            >
              {connecting
                ? t("notes.settings.notion.connecting")
                : t("notes.settings.notion.connect")}
            </Button>
          </div>
        </form>
      )}
    </SettingsGroup>
  );
};
