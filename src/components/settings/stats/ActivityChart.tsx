import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DailyUsage } from "@/bindings";
import { barHeight, parseLocalDay } from "./format";

interface ActivityChartProps {
  daily: DailyUsage[];
}

/** 30 thin bars, one per day. Neutral bars, today in the accent color; hover
 * a day to read its count in the caption (no floating tooltip to keep the
 * chart calm). */
export const ActivityChart: React.FC<ActivityChartProps> = ({ daily }) => {
  const { t, i18n } = useTranslation();
  const [hovered, setHovered] = useState<number | null>(null);

  const max = useMemo(
    () => daily.reduce((acc, d) => Math.max(acc, d.words), 0),
    [daily],
  );

  const dateFormat = useMemo(
    () =>
      new Intl.DateTimeFormat(i18n.language, {
        weekday: "short",
        day: "numeric",
        month: "short",
      }),
    [i18n.language],
  );
  const numberFormat = useMemo(
    () => new Intl.NumberFormat(i18n.language),
    [i18n.language],
  );

  const describe = (day: DailyUsage): string => {
    const parsed = parseLocalDay(day.day);
    const date = parsed ? dateFormat.format(parsed) : day.day;
    if (day.words === 0) return t("settings.stats.chart.noWords", { date });
    return t("settings.stats.chart.tooltip", {
      date,
      count: day.words,
      formatted: numberFormat.format(day.words),
    });
  };

  const lastIndex = daily.length - 1;
  const caption =
    hovered !== null && daily[hovered]
      ? describe(daily[hovered])
      : t("settings.stats.chart.title");

  return (
    <div className="space-y-2">
      <p
        className={`text-xs tabular-nums transition-colors ${
          hovered !== null ? "text-text" : "text-mid-gray"
        }`}
        aria-live="polite"
      >
        {caption}
      </p>
      <div
        className="flex items-end gap-[3px] h-20"
        role="group"
        aria-label={t("settings.stats.chart.title")}
        onMouseLeave={() => setHovered(null)}
      >
        {daily.map((day, index) => {
          const height = barHeight(day.words, max);
          const isToday = index === lastIndex;
          const isHovered = hovered === index;
          let color = "bg-mid-gray/20";
          if (height > 0) {
            color = isToday ? "bg-logo-primary" : "bg-text/25";
          }
          if (isHovered) {
            color = height > 0 ? "bg-logo-primary/70" : "bg-mid-gray/50";
          }
          return (
            <div
              key={day.day}
              className="flex-1 h-full flex items-end cursor-default"
              onMouseEnter={() => setHovered(index)}
              aria-label={describe(day)}
              role="img"
            >
              <div
                className={`w-full rounded-[2px] transition-colors ${color}`}
                style={{ height: height > 0 ? `${height}%` : "2px" }}
              />
            </div>
          );
        })}
      </div>
      <div className="flex justify-between text-[11px] text-mid-gray">
        <span>{t("settings.stats.chart.start")}</span>
        <span>{t("settings.stats.chart.today")}</span>
      </div>
    </div>
  );
};
