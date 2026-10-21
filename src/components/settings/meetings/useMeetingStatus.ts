import { useEffect, useState } from "react";
import { commands, events, type MeetingStatus } from "@/bindings";

/** Live elapsed time of a status snapshot. */
export const liveElapsed = (status: MeetingStatus | null, now: number) => {
  if (!status) return 0;
  if (status.phase !== "recording") return status.elapsed_ms;
  return status.elapsed_ms + Math.max(0, now - status.as_of);
};

/**
 * The meeting in progress, kept current from backend events, with an elapsed
 * time that ticks every second while recording.
 */
export const useMeetingStatus = () => {
  const [status, setStatus] = useState<MeetingStatus | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    let cancelled = false;
    commands
      .getMeetingStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((e) => console.error("Failed to read meeting status:", e));
    const unlisten = events.meetingStatusEvent.listen((event) => {
      setStatus(event.payload);
      setNow(Date.now());
    });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  const recording = status?.phase === "recording";
  useEffect(() => {
    if (!recording) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [recording]);

  return { status, elapsedMs: liveElapsed(status, now) };
};
