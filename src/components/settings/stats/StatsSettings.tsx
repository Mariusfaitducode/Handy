import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { ChartColumn } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type UsageShare, type UsageStats } from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import { Button } from "../../ui/Button";
import { Dialog } from "../../ui/Dialog";
import { Input } from "../../ui/Input";
import { SettingContainer } from "../../ui/SettingContainer";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ActivityChart } from "./ActivityChart";
import {
  durationParts,
  languageName,
  parseLocalDay,
  percentage,
} from "./format";

/** Emitted by the backend whenever a dictation is recorded or stats change. */
const STATS_CHANGED_EVENT = "usage-stats-changed";

const MIN_TYPING_WPM = 10;
const MAX_TYPING_WPM = 200;

type LoadState =
  | { status: "loading" }
  | { status: "error" }
  | { status: "ready"; stats: UsageStats };

const useDuration = () => {
  const { t } = useTranslation();
  return useCallback(
    (seconds: number) => {
      const parts = durationParts(seconds);
      switch (parts.kind) {
        case "lessThanMinute":
          return t("settings.stats.duration.lessThanMinute");
        case "minutes":
          return t("settings.stats.duration.minutes", {
            minutes: parts.minutes,
          });
        case "hours":
          return t("settings.stats.duration.hours", { hours: parts.hours });
        case "hoursMinutes":
          return t("settings.stats.duration.hoursMinutes", {
            hours: parts.hours,
            minutes: parts.minutes,
          });
      }
    },
    [t],
  );
};

export const StatsSettings: React.FC = () => {
  const { t } = useTranslation();
  const [state, setState] = useState<LoadState>({ status: "loading" });

  const load = useCallback(async () => {
    try {
      const result = await commands.getUsageStats();
      if (result.status === "ok") {
        setState({ status: "ready", stats: result.data });
      } else {
        console.error("Failed to load usage stats:", result.error);
        // Keep showing the last good numbers on a failed refresh.
        setState((prev) =>
          prev.status === "ready" ? prev : { status: "error" },
        );
      }
    } catch (error) {
      console.error("Failed to load usage stats:", error);
      setState((prev) =>
        prev.status === "ready" ? prev : { status: "error" },
      );
    }
  }, []);

  useEffect(() => {
    load();
    const unlisten = listen(STATS_CHANGED_EVENT, () => {
      load();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [load]);

  let content: React.ReactNode;
  if (state.status === "loading") {
    content = <StatsSkeleton />;
  } else if (state.status === "error") {
    content = (
      <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-8 flex flex-col items-center gap-3 text-center">
        <p className="text-sm text-text/70">{t("settings.stats.error")}</p>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => {
            setState({ status: "loading" });
            load();
          }}
        >
          {t("settings.stats.retry")}
        </Button>
      </div>
    );
  } else if (state.stats.total_dictations === 0) {
    content = <EmptyState />;
  } else {
    content = <StatsContent stats={state.stats} onChanged={load} />;
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("settings.stats.title")}
          </h2>
        </div>
        {content}
      </div>
    </div>
  );
};

const StatsSkeleton: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="space-y-4" aria-busy="true">
      <span className="sr-only">{t("settings.stats.loading")}</span>
      <div className="h-52 rounded-lg border border-mid-gray/20 bg-mid-gray/5 animate-pulse" />
      <div className="h-[72px] rounded-lg border border-mid-gray/20 bg-mid-gray/5 animate-pulse" />
    </div>
  );
};

const EmptyState: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="bg-background border border-mid-gray/20 rounded-lg px-6 py-12 flex flex-col items-center text-center gap-3">
      <div className="flex items-end gap-[3px] h-8 opacity-60" aria-hidden>
        {[30, 55, 40, 75, 50, 100, 65].map((h, i) => (
          <div
            key={i}
            className="w-1.5 rounded-[2px] bg-mid-gray/30"
            style={{ height: `${h}%` }}
          />
        ))}
      </div>
      <h3 className="text-sm font-medium">{t("settings.stats.empty.title")}</h3>
      <p className="text-sm text-mid-gray max-w-xs">
        {t("settings.stats.empty.description")}
      </p>
    </div>
  );
};

interface StatsContentProps {
  stats: UsageStats;
  onChanged: () => void;
}

const StatsContent: React.FC<StatsContentProps> = ({ stats, onChanged }) => {
  const { t, i18n } = useTranslation();
  const osType = useOsType();
  const formatDuration = useDuration();
  const numberFormat = useMemo(
    () => new Intl.NumberFormat(i18n.language),
    [i18n.language],
  );
  const n = (value: number) => numberFormat.format(value);

  const since = useMemo(() => {
    const day = stats.first_day ? parseLocalDay(stats.first_day) : null;
    if (!day) return null;
    return new Intl.DateTimeFormat(i18n.language, {
      day: "numeric",
      month: "long",
      year: "numeric",
    }).format(day);
  }, [stats.first_day, i18n.language]);

  // App names only come from macOS; elsewhere the section would stay empty.
  const showApps = osType === "macos" || stats.app_words > 0;

  return (
    <div className="space-y-4">
      {/* Hero: total words, time saved, 30-day activity */}
      <div className="bg-background border border-mid-gray/20 rounded-lg px-5 pt-5 pb-4 space-y-5">
        <div>
          <div className="flex items-baseline gap-2 flex-wrap">
            <span className="text-4xl font-semibold tabular-nums tracking-tight leading-none">
              {n(stats.total_words)}
            </span>
            <span className="text-sm text-mid-gray">
              {t("settings.stats.hero.words", { count: stats.total_words })}
            </span>
          </div>
          <p className="text-sm text-text/70 mt-2">
            {stats.time_saved_seconds > 0
              ? t("settings.stats.hero.timeSaved", {
                  duration: formatDuration(stats.time_saved_seconds),
                  wpm: stats.typing_wpm,
                })
              : t("settings.stats.hero.noTimeSaved")}
          </p>
        </div>
        <ActivityChart daily={stats.daily} />
      </div>

      {/* Key figures */}
      <div className="grid grid-cols-4 bg-background border border-mid-gray/20 rounded-lg divide-x divide-mid-gray/20">
        <Tile
          label={t("settings.stats.tiles.today")}
          value={n(stats.today_words)}
        />
        <Tile
          label={t("settings.stats.tiles.week")}
          value={n(stats.week_words)}
        />
        <Tile
          label={t("settings.stats.tiles.streak")}
          value={t("settings.stats.tiles.streakValue", {
            count: stats.current_streak_days,
          })}
          hint={
            stats.longest_streak_days > stats.current_streak_days
              ? t("settings.stats.tiles.streakBest", {
                  count: stats.longest_streak_days,
                })
              : undefined
          }
        />
        <Tile
          label={t("settings.stats.tiles.pace")}
          value={
            stats.speaking_wpm !== null
              ? n(stats.speaking_wpm)
              : t("settings.stats.tiles.unknown")
          }
          unit={
            stats.speaking_wpm !== null
              ? t("settings.stats.tiles.paceUnit")
              : undefined
          }
          hint={
            stats.speaking_wpm === null
              ? t("settings.stats.tiles.paceUnknownHint")
              : undefined
          }
        />
      </div>

      {/* Breakdowns */}
      <div className={`grid gap-4 ${showApps ? "grid-cols-2" : "grid-cols-1"}`}>
        {showApps && (
          <Breakdown
            title={t("settings.stats.apps.title")}
            items={stats.top_apps}
            total={stats.app_words}
            emptyText={t("settings.stats.apps.empty")}
            label={(name) => name}
          />
        )}
        <Breakdown
          title={t("settings.stats.languages.title")}
          items={stats.languages}
          total={stats.language_words}
          emptyText={t("settings.stats.languages.empty")}
          label={(code) => languageName(code, i18n.language)}
        />
      </div>

      <SettingsGroup title={t("settings.stats.settings.title")}>
        <TypingSpeedSetting value={stats.typing_wpm} onChanged={onChanged} />
        <ResetStatsSetting onChanged={onChanged} />
      </SettingsGroup>

      <p className="text-xs text-mid-gray text-center px-4">
        {t("settings.stats.footer.privacy")}
        {since && (
          <>
            {" · "}
            {t("settings.stats.footer.since", { date: since })}
          </>
        )}
      </p>
    </div>
  );
};

interface TileProps {
  label: string;
  value: string;
  unit?: string;
  hint?: string;
}

const Tile: React.FC<TileProps> = ({ label, value, unit, hint }) => (
  <div className="px-4 py-3 min-w-0">
    <p className="text-xs text-mid-gray truncate" title={label}>
      {label}
    </p>
    <p className="mt-0.5 flex items-baseline gap-1 min-w-0">
      <span className="text-lg font-semibold tabular-nums truncate">
        {value}
      </span>
      {unit && <span className="text-xs text-mid-gray truncate">{unit}</span>}
    </p>
    {hint && (
      <p
        className="text-[11px] text-mid-gray leading-tight truncate"
        title={hint}
      >
        {hint}
      </p>
    )}
  </div>
);

interface BreakdownProps {
  title: string;
  items: UsageShare[];
  total: number;
  emptyText: string;
  label: (name: string) => string;
}

const Breakdown: React.FC<BreakdownProps> = ({
  title,
  items,
  total,
  emptyText,
  label,
}) => (
  <div className="space-y-2 min-w-0">
    <h3 className="px-4 text-xs font-medium text-mid-gray uppercase tracking-wide">
      {title}
    </h3>
    <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-3">
      {items.length === 0 ? (
        <p className="text-xs text-mid-gray py-1">{emptyText}</p>
      ) : (
        <ul className="space-y-2.5">
          {items.map((item) => {
            const share = percentage(item.words, total);
            const name = label(item.name);
            return (
              <li key={item.name} className="space-y-1">
                <div className="flex justify-between gap-2 text-sm">
                  <span className="truncate" title={name}>
                    {name}
                  </span>
                  <span className="text-mid-gray tabular-nums shrink-0">
                    {share}%
                  </span>
                </div>
                <div className="h-1 rounded-full bg-mid-gray/15 overflow-hidden">
                  <div
                    className="h-full rounded-full bg-logo-primary/70"
                    style={{ width: `${share}%` }}
                  />
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  </div>
);

const TypingSpeedSetting: React.FC<{
  value: number;
  onChanged: () => void;
}> = ({ value, onChanged }) => {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(String(value));
  const [saving, setSaving] = useState(false);
  const focused = useRef(false);

  // Follow external updates unless the user is typing.
  useEffect(() => {
    if (!focused.current) setDraft(String(value));
  }, [value]);

  const commit = async () => {
    const parsed = parseInt(draft, 10);
    if (isNaN(parsed)) {
      setDraft(String(value));
      return;
    }
    const clamped = Math.min(MAX_TYPING_WPM, Math.max(MIN_TYPING_WPM, parsed));
    setDraft(String(clamped));
    if (clamped === value) return;
    setSaving(true);
    try {
      const result = await commands.setStatsTypingSpeed(clamped);
      if (result.status === "ok") {
        setDraft(String(result.data));
        onChanged();
      } else {
        setDraft(String(value));
      }
    } catch (error) {
      console.error("Failed to save typing speed:", error);
      setDraft(String(value));
    } finally {
      setSaving(false);
    }
  };

  return (
    <SettingContainer
      title={t("settings.stats.settings.typingSpeed.title")}
      description={t("settings.stats.settings.typingSpeed.description")}
      descriptionMode="tooltip"
      grouped={true}
      layout="horizontal"
    >
      <div className="flex items-center gap-2">
        <Input
          type="number"
          min={MIN_TYPING_WPM}
          max={MAX_TYPING_WPM}
          step={5}
          value={draft}
          disabled={saving}
          onFocus={() => {
            focused.current = true;
          }}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => {
            focused.current = false;
            commit();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") {
              setDraft(String(value));
              (e.target as HTMLInputElement).blur();
            }
          }}
          className="w-20"
        />
        <span className="text-sm text-text">
          {t("settings.stats.settings.typingSpeed.unit")}
        </span>
      </div>
    </SettingContainer>
  );
};

const ResetStatsSetting: React.FC<{ onChanged: () => void }> = ({
  onChanged,
}) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [resetting, setResetting] = useState(false);

  const reset = async () => {
    setResetting(true);
    try {
      const result = await commands.resetUsageStats();
      if (result.status !== "ok") throw new Error(String(result.error));
      setOpen(false);
      onChanged();
    } catch (error) {
      console.error("Failed to reset stats:", error);
      toast.error(t("settings.stats.settings.reset.error"));
    } finally {
      setResetting(false);
    }
  };

  return (
    <>
      <SettingContainer
        title={t("settings.stats.settings.reset.title")}
        description={t("settings.stats.settings.reset.description")}
        descriptionMode="tooltip"
        grouped={true}
        layout="horizontal"
      >
        <Button variant="danger-ghost" size="sm" onClick={() => setOpen(true)}>
          {t("settings.stats.settings.reset.button")}
        </Button>
      </SettingContainer>
      <Dialog
        open={open}
        onOpenChange={(next) => !resetting && setOpen(next)}
        title={t("settings.stats.settings.reset.dialogTitle")}
        closeLabel={t("common.close")}
        footer={
          <div className="flex justify-end gap-2">
            <Button
              variant="secondary"
              size="md"
              onClick={() => setOpen(false)}
              disabled={resetting}
            >
              {t("settings.stats.settings.reset.cancel")}
            </Button>
            <Button
              variant="danger"
              size="md"
              onClick={reset}
              disabled={resetting}
            >
              {t("settings.stats.settings.reset.confirm")}
            </Button>
          </div>
        }
      >
        <p className="text-sm text-text/80">
          {t("settings.stats.settings.reset.dialogBody")}
        </p>
      </Dialog>
    </>
  );
};
