import React, { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Download, FolderSync, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type DictionarySnapshot } from "@/bindings";
import { formatRelativeTime } from "@/utils/dateFormat";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import { SettingContainer } from "../../ui/SettingContainer";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { friendlyFolderPath } from "./useDictionary";

const FILE_FILTER_EXTENSIONS = ["json"];

interface SharingSettingsProps {
  snapshot: DictionarySnapshot;
  onChange: (snapshot: DictionarySnapshot) => void;
}

/** Re-renders periodically so relative times stay true. */
const useNow = (intervalMs: number) => {
  const [, setNow] = useState(Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
};

export const SharingSettings: React.FC<SharingSettingsProps> = ({
  snapshot,
  onChange,
}) => {
  const { t } = useTranslation();

  return (
    <SettingsGroup
      title={t("settings.dictionary.sharing.title")}
      description={t("settings.dictionary.sharing.description")}
    >
      <AuthorSetting snapshot={snapshot} onChange={onChange} />
      <FolderSetting snapshot={snapshot} onChange={onChange} />
      <FileSetting onChange={onChange} />
    </SettingsGroup>
  );
};

const AuthorSetting: React.FC<SharingSettingsProps> = ({
  snapshot,
  onChange,
}) => {
  const { t } = useTranslation();
  const [name, setName] = useState(snapshot.author);

  useEffect(() => setName(snapshot.author), [snapshot.author]);

  const commit = async () => {
    const trimmed = name.trim();
    if (!trimmed) {
      setName(snapshot.author);
      return;
    }
    if (trimmed === snapshot.author) return;
    const result = await commands.setDictionaryAuthor(trimmed);
    if (result.status === "ok") onChange(result.data);
    else setName(snapshot.author);
  };

  return (
    <SettingContainer
      title={t("settings.dictionary.sharing.author.title")}
      description={t("settings.dictionary.sharing.author.description")}
      descriptionMode="tooltip"
      grouped
    >
      <Input
        type="text"
        variant="compact"
        className="w-40"
        value={name}
        maxLength={40}
        placeholder={t("settings.dictionary.sharing.author.placeholder")}
        onChange={(e) => setName(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") setName(snapshot.author);
        }}
      />
    </SettingContainer>
  );
};

const FolderSetting: React.FC<SharingSettingsProps> = ({
  snapshot,
  onChange,
}) => {
  const { t, i18n } = useTranslation();
  const [syncing, setSyncing] = useState(false);
  // Keeps "synced 2 minutes ago" current.
  useNow(30_000);
  const { folder, last_sync_at, error } = snapshot.sync;

  const run = async (
    action: () => Promise<
      | { status: "ok"; data: DictionarySnapshot }
      | { status: "error"; error: string }
    >,
  ) => {
    setSyncing(true);
    try {
      const result = await action();
      if (result.status === "ok") onChange(result.data);
    } catch (e) {
      console.error("Dictionary sync failed:", e);
    } finally {
      setSyncing(false);
    }
  };

  const choose = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: t("settings.dictionary.sharing.folder.dialogTitle"),
    });
    if (typeof picked === "string") {
      run(() => commands.setDictionarySyncFolder(picked));
    }
  };

  const stop = () =>
    run(async () => {
      const result = await commands.setDictionarySyncFolder(null);
      if (result.status === "ok") {
        toast(t("settings.dictionary.sharing.folder.stopped"));
      }
      return result;
    });

  let status: React.ReactNode = null;
  if (folder) {
    if (syncing) {
      status = t("settings.dictionary.sharing.folder.syncing");
    } else if (error) {
      status = (
        <span className="text-warning">
          {t(`settings.dictionary.sharing.syncErrors.${error.kind}`)}
        </span>
      );
    } else if (last_sync_at) {
      const seconds = Math.floor(last_sync_at / 1000);
      status = t("settings.dictionary.sharing.folder.lastSync", {
        time: formatRelativeTime(String(seconds), i18n.language),
      });
    } else {
      status = t("settings.dictionary.sharing.folder.never");
    }
  }

  return (
    <SettingContainer
      title={t("settings.dictionary.sharing.folder.title")}
      description={t("settings.dictionary.sharing.folder.description")}
      descriptionMode="tooltip"
      grouped
      layout={folder ? "stacked" : "horizontal"}
    >
      {folder ? (
        <div className="flex items-center gap-3">
          <FolderSync className="w-4 h-4 shrink-0 text-mid-gray" aria-hidden />
          <div className="flex-1 min-w-0">
            <p className="text-sm truncate" title={folder}>
              {friendlyFolderPath(folder)}
            </p>
            <p className="text-xs text-mid-gray" aria-live="polite">
              {status}
            </p>
          </div>
          <Button
            variant="secondary"
            size="sm"
            disabled={syncing}
            onClick={() => run(() => commands.syncDictionaryNow())}
          >
            {t("settings.dictionary.sharing.folder.syncNow")}
          </Button>
          <Button variant="ghost" size="sm" disabled={syncing} onClick={stop}>
            {t("settings.dictionary.sharing.folder.stop")}
          </Button>
        </div>
      ) : (
        <Button variant="secondary" size="sm" onClick={choose}>
          {t("settings.dictionary.sharing.folder.choose")}
        </Button>
      )}
    </SettingContainer>
  );
};

const FileSetting: React.FC<{
  onChange: (snapshot: DictionarySnapshot) => void;
}> = ({ onChange }) => {
  const { t } = useTranslation();
  const filters = [
    {
      name: t("settings.dictionary.sharing.file.filterName"),
      extensions: FILE_FILTER_EXTENSIONS,
    },
  ];

  const exportFile = async () => {
    const path = await save({
      defaultPath: "handy-dictionary.json",
      filters,
    });
    if (!path) return;
    try {
      const result = await commands.exportDictionary(path);
      if (result.status === "ok") {
        toast.success(
          t("settings.dictionary.sharing.file.exported", {
            count: result.data,
          }),
        );
      } else {
        toast.error(t("settings.dictionary.sharing.file.exportError"));
      }
    } catch (e) {
      console.error("Dictionary export failed:", e);
      toast.error(t("settings.dictionary.sharing.file.exportError"));
    }
  };

  const importFile = async () => {
    const path = await open({ multiple: false, directory: false, filters });
    if (typeof path !== "string") return;
    try {
      const result = await commands.importDictionary(path);
      if (result.status !== "ok") {
        toast.error(t("settings.dictionary.sharing.file.importError"));
        return;
      }
      onChange(result.data.snapshot);
      const { added, updated } = result.data.stats;
      if (added + updated === 0) {
        toast(t("settings.dictionary.sharing.file.importedNothing"));
      } else {
        toast.success(
          t("settings.dictionary.sharing.file.imported", { added, updated }),
        );
      }
    } catch (e) {
      console.error("Dictionary import failed:", e);
      toast.error(t("settings.dictionary.sharing.file.importError"));
    }
  };

  return (
    <SettingContainer
      title={t("settings.dictionary.sharing.file.title")}
      description={t("settings.dictionary.sharing.file.description")}
      descriptionMode="tooltip"
      grouped
    >
      <div className="flex items-center gap-2">
        <Button
          variant="secondary"
          size="sm"
          className="inline-flex items-center gap-1.5"
          onClick={exportFile}
        >
          <Upload className="w-3.5 h-3.5" aria-hidden />
          {t("settings.dictionary.sharing.file.export")}
        </Button>
        <Button
          variant="secondary"
          size="sm"
          className="inline-flex items-center gap-1.5"
          onClick={importFile}
        >
          <Download className="w-3.5 h-3.5" aria-hidden />
          {t("settings.dictionary.sharing.file.import")}
        </Button>
      </div>
    </SettingContainer>
  );
};
