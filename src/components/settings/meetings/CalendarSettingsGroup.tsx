import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type CalendarInfo,
  type CalendarMode,
  type CalendarOverview,
  type CalendarSettings,
} from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { Dropdown } from "../../ui/Dropdown";
import { SettingContainer } from "../../ui/SettingContainer";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ToggleSwitch } from "../../ui/ToggleSwitch";

const DEFAULTS: CalendarSettings = {
  mode: "off",
  excluded_calendars: [],
  only_video_calls: true,
  skip_without_attendees: true,
  skipped_events: [],
  included_events: [],
};

const MODES: CalendarMode[] = ["ask", "auto", "off"];

/** Calendar auto-start preferences, once Handy can read the calendar. */
export const CalendarSettingsGroup: React.FC<{
  overview: CalendarOverview | null;
}> = ({ overview }) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const settings = getSetting("calendar") ?? DEFAULTS;
  const updating = isUpdating("calendar");
  const [calendars, setCalendars] = useState<CalendarInfo[] | null>(null);
  const granted = overview?.access === "granted";

  useEffect(() => {
    if (!granted) return;
    commands.listCalendars().then((result) => {
      if (result.status === "ok") setCalendars(result.data);
    });
  }, [granted]);

  const update = (patch: Partial<CalendarSettings>) =>
    updateSetting("calendar", { ...settings, ...patch });

  // Calendars grouped by account, in the order the backend sorted them.
  const accounts = useMemo(() => {
    const out: { source: string; items: CalendarInfo[] }[] = [];
    for (const calendar of calendars ?? []) {
      const last = out[out.length - 1];
      if (last && last.source === calendar.source) last.items.push(calendar);
      else out.push({ source: calendar.source, items: [calendar] });
    }
    return out;
  }, [calendars]);

  if (!granted) return null;

  const toggleCalendar = (id: string, followed: boolean) => {
    const excluded = settings.excluded_calendars.filter((c) => c !== id);
    if (!followed) excluded.push(id);
    update({ excluded_calendars: excluded });
  };

  const off = settings.mode === "off";

  return (
    <SettingsGroup title={t("meetings.calendar.settings.title")}>
      <SettingContainer
        title={t("meetings.calendar.settings.mode.label")}
        description={t("meetings.calendar.settings.mode.description")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <Dropdown
          options={MODES.map((value) => ({
            value,
            label: t(`meetings.calendar.settings.mode.${value}`),
          }))}
          selectedValue={settings.mode}
          onSelect={(value) => update({ mode: value as CalendarMode })}
          disabled={updating}
        />
      </SettingContainer>
      <ToggleSwitch
        checked={settings.only_video_calls}
        onChange={(only_video_calls) => update({ only_video_calls })}
        disabled={off}
        isUpdating={updating}
        label={t("meetings.calendar.settings.onlyVideo.label")}
        description={t("meetings.calendar.settings.onlyVideo.description")}
        descriptionMode="tooltip"
        grouped={true}
      />
      <ToggleSwitch
        checked={settings.skip_without_attendees}
        onChange={(skip_without_attendees) =>
          update({ skip_without_attendees })
        }
        disabled={off}
        isUpdating={updating}
        label={t("meetings.calendar.settings.skipAlone.label")}
        description={t("meetings.calendar.settings.skipAlone.description")}
        descriptionMode="tooltip"
        grouped={true}
      />
      <SettingContainer
        title={t("meetings.calendar.settings.calendars.label")}
        description={t("meetings.calendar.settings.calendars.description")}
        descriptionMode="tooltip"
        layout="stacked"
        grouped={true}
        disabled={off}
      >
        {calendars === null ? null : calendars.length === 0 ? (
          <p className="text-xs text-mid-gray">
            {t("meetings.calendar.settings.calendars.none")}
          </p>
        ) : (
          <div className="flex flex-col gap-3 pt-1">
            {accounts.map((account) => (
              <div key={account.source} className="flex flex-col gap-1">
                {account.source && (
                  <p className="text-xs text-mid-gray">{account.source}</p>
                )}
                {account.items.map((calendar) => {
                  const followed = !settings.excluded_calendars.includes(
                    calendar.id,
                  );
                  return (
                    <label
                      key={calendar.id}
                      className="flex items-center gap-2 text-sm cursor-pointer"
                    >
                      <input
                        type="checkbox"
                        checked={followed}
                        disabled={off || updating}
                        onChange={(e) =>
                          toggleCalendar(calendar.id, e.target.checked)
                        }
                        className="accent-logo-primary"
                      />
                      <span
                        className="w-2 h-2 rounded-full shrink-0"
                        style={{
                          backgroundColor: calendar.color ?? "transparent",
                        }}
                        aria-hidden
                      />
                      <span className="truncate">{calendar.title}</span>
                    </label>
                  );
                })}
              </div>
            ))}
          </div>
        )}
      </SettingContainer>
    </SettingsGroup>
  );
};
