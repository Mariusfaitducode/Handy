import { create } from "zustand";

/**
 * Which meeting the Meetings section shows. Lives outside the section so the
 * recording indicator (another window) and the tray can open a meeting in
 * the main window.
 */
interface MeetingNavigationStore {
  openMeetingId: number | null;
  openMeeting: (id: number | null) => void;
}

export const useMeetingNavigation = create<MeetingNavigationStore>((set) => ({
  openMeetingId: null,
  openMeeting: (id) => set({ openMeetingId: id }),
}));

/** Event the indicator window emits to show a meeting in the main window. */
export const OPEN_MEETING_EVENT = "meeting-open";
