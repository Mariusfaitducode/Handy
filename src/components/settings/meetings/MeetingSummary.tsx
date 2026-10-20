import React from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import { useTranslation } from "react-i18next";
import type { MeetingRecord } from "@/bindings";
import { Button } from "../../ui/Button";

const allowedElements = [
  "br",
  "code",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "hr",
  "li",
  "ol",
  "p",
  "strong",
  "ul",
  "blockquote",
];

/** A "- [ ] task" item: react-markdown without GFM leaves "[ ] " as text. */
const TaskAware: React.FC<{ children?: React.ReactNode }> = ({ children }) => {
  const items = React.Children.toArray(children);
  const first = items[0];
  if (typeof first === "string") {
    const match = /^\[( |x|X)\]\s+/.exec(first);
    if (match) {
      const done = match[1] !== " ";
      return (
        <li className="flex gap-2 items-baseline list-none -ms-5">
          <span
            aria-hidden
            className={`inline-block w-3 h-3 shrink-0 rounded-[3px] border translate-y-[1px] ${
              done ? "bg-mid-gray/60 border-mid-gray/60" : "border-mid-gray/70"
            }`}
          />
          <span>
            {first.slice(match[0].length)}
            {items.slice(1)}
          </span>
        </li>
      );
    }
  }
  return <li className="ps-1 marker:text-text/40">{children}</li>;
};

const components: Components = {
  h1: ({ children }) => (
    <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide pt-2">
      {children}
    </h3>
  ),
  h2: ({ children }) => (
    <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide pt-2">
      {children}
    </h3>
  ),
  h3: ({ children }) => (
    <h4 className="text-sm font-semibold pt-1">{children}</h4>
  ),
  h4: ({ children }) => (
    <h4 className="text-sm font-semibold pt-1">{children}</h4>
  ),
  p: ({ children }) => (
    <p className="text-sm leading-relaxed text-text/90">{children}</p>
  ),
  ul: ({ children }) => (
    <ul className="list-disc space-y-1.5 ps-5 text-sm leading-relaxed text-text/90">
      {children}
    </ul>
  ),
  ol: ({ children }) => (
    <ol className="list-decimal space-y-1.5 ps-5 text-sm leading-relaxed text-text/90">
      {children}
    </ol>
  ),
  li: ({ children }) => <TaskAware>{children}</TaskAware>,
  strong: ({ children }) => (
    <strong className="font-semibold text-text">{children}</strong>
  ),
  hr: () => <hr className="border-mid-gray/20" />,
  blockquote: ({ children }) => (
    <blockquote className="border-s-2 border-mid-gray/40 ps-3 text-sm text-text/70">
      {children}
    </blockquote>
  ),
  code: ({ children }) => (
    <code className="rounded bg-mid-gray/10 px-1 py-0.5 font-mono text-[0.85em]">
      {children}
    </code>
  ),
};

interface MeetingSummaryProps {
  record: MeetingRecord;
  live: boolean;
  onGenerate: () => void;
  onOpenSettings: () => void;
}

/** The minutes, or why there are none and what to do about it. */
export const MeetingSummary: React.FC<MeetingSummaryProps> = ({
  record,
  live,
  onGenerate,
  onOpenSettings,
}) => {
  const { t } = useTranslation();
  const hasText = !!record.summary?.trim();

  if (live || record.status === "processing") {
    return <Muted>{t("meetings.summary.inProgress")}</Muted>;
  }

  if (record.summary_status === "pending") {
    return (
      <div className="flex items-center gap-3 py-2">
        <div className="w-3.5 h-3.5 border-2 border-mid-gray/60 border-t-transparent rounded-full animate-spin" />
        <p
          className="text-sm text-text/70"
          style={{ animation: "meeting-pulse 2.4s ease-in-out infinite" }}
        >
          {t("meetings.summary.pending")}
        </p>
        <style>{`@keyframes meeting-pulse { 0%,100% { opacity: .55 } 50% { opacity: 1 } }`}</style>
      </div>
    );
  }

  if (record.summary_status === "not_configured" && !hasText) {
    return (
      <div className="space-y-2 py-1">
        <p className="text-sm font-medium">
          {t("meetings.summary.notConfiguredTitle")}
        </p>
        <p className="text-sm text-text/70 leading-relaxed">
          {t("meetings.summary.notConfiguredBody")}
        </p>
        <div className="flex gap-2 pt-1">
          <Button variant="secondary" size="sm" onClick={onOpenSettings}>
            {t("meetings.summary.openSettings")}
          </Button>
          <Button variant="ghost" size="sm" onClick={onGenerate}>
            {t("meetings.summary.retry")}
          </Button>
        </div>
      </div>
    );
  }

  const failed = record.summary_status === "failed";
  return (
    <div className="space-y-3">
      {failed && (
        <div className="flex items-start justify-between gap-3 rounded-md bg-mid-gray/10 px-3 py-2">
          <div className="min-w-0">
            <p className="text-sm">{t("meetings.summary.failed")}</p>
            {record.summary_error && (
              <p className="text-xs text-mid-gray break-words">
                {record.summary_error}
              </p>
            )}
          </div>
          <Button variant="secondary" size="sm" onClick={onGenerate}>
            {t("meetings.summary.retry")}
          </Button>
        </div>
      )}

      {hasText ? (
        <>
          <div className="space-y-2">
            <ReactMarkdown
              allowedElements={allowedElements}
              components={components}
              skipHtml
            >
              {record.summary ?? ""}
            </ReactMarkdown>
          </div>
          <div className="flex items-center justify-between pt-2 border-t border-mid-gray/20">
            <p className="text-xs text-mid-gray">
              {record.summary_model
                ? t("meetings.summary.writtenBy", {
                    model: record.summary_model,
                  })
                : null}
            </p>
            <Button variant="ghost" size="sm" onClick={onGenerate}>
              {t("meetings.summary.regenerate")}
            </Button>
          </div>
        </>
      ) : (
        !failed && (
          <div className="space-y-2">
            <Muted>
              {record.summary_status === "empty"
                ? t("meetings.summary.empty")
                : t("meetings.summary.none")}
            </Muted>
            {record.summary_status !== "empty" && (
              <Button variant="secondary" size="sm" onClick={onGenerate}>
                {t("meetings.summary.generate")}
              </Button>
            )}
          </div>
        )
      )}
    </div>
  );
};

const Muted: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <p className="text-sm text-text/60 leading-relaxed py-1">{children}</p>
);
