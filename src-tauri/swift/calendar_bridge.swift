// Calendar bridge for meeting auto-start: reads the macOS calendar database
// through EventKit, which covers every account added to Calendar.app
// (iCloud, Google, Exchange, CalDAV…).
//
// Everything crosses the C boundary as JSON strings so the Rust side owns the
// logic (video-link detection, filtering) and can unit-test it. Read-only:
// Handy never writes to the calendar.

import CoreGraphics
import EventKit
import Foundation
import UserNotifications

private let lock = NSLock()
private var sharedStore = EKEventStore()

/// Notes longer than this are cut: a video link sits near the top of an
/// invitation, and some invitations carry pages of legal text.
private let maxNotesLength = 8_000

// 0 not determined, 1 restricted, 2 denied, 3 full access (`.authorized`
// before macOS 14), 4 write only. These are EKAuthorizationStatus's raw
// values; going through them avoids availability errors on `.writeOnly`.
private func statusCode(_ status: EKAuthorizationStatus) -> Int32 {
    let raw = Int32(status.rawValue)
    return (0...4).contains(raw) ? raw : 2
}

@_cdecl("handy_calendar_authorization")
public func handyCalendarAuthorization() -> Int32 {
    return statusCode(EKEventStore.authorizationStatus(for: .event))
}

/// Shows the system prompt (only the first time) and blocks until the user
/// answers. Must not be called on the main thread.
@_cdecl("handy_calendar_request_access")
public func handyCalendarRequestAccess() -> Int32 {
    let semaphore = DispatchSemaphore(value: 0)
    lock.lock()
    let store = sharedStore
    lock.unlock()
    let done: (Bool, Error?) -> Void = { _, error in
        if let error = error {
            NSLog("Handy calendar access request failed: \(error.localizedDescription)")
        }
        semaphore.signal()
    }
    if #available(macOS 14.0, *) {
        store.requestFullAccessToEvents(completion: done)
    } else {
        store.requestAccess(to: .event, completion: done)
    }
    semaphore.wait()
    // A store created before access was granted doesn't see any event.
    lock.lock()
    sharedStore = EKEventStore()
    lock.unlock()
    return handyCalendarAuthorization()
}

private struct CalendarInfo: Codable {
    let id: String
    let title: String
    let source: String
    let color: String?
}

private struct Participant: Codable {
    let name: String?
    let email: String?
    /// EKParticipantStatus raw value (2 accepted, 3 declined, 4 tentative…).
    let status: Int
    /// EKParticipantType raw value (1 person, 2 room, 3 resource, 4 group).
    let kind: Int
    let is_current_user: Bool
}

private struct EventInfo: Codable {
    let series_id: String
    let title: String
    let start_ms: Int64
    let end_ms: Int64
    let all_day: Bool
    let location: String?
    let notes: String?
    let url: String?
    let calendar_id: String
    let calendar_title: String
    let calendar_color: String?
    /// EKEventStatus raw value (3 canceled).
    let status: Int
    let recurring: Bool
    let organizer: Participant?
    let attendees: [Participant]
}

private func hexColor(_ color: CGColor?) -> String? {
    guard let color = color,
          let rgb = color.converted(to: CGColorSpace(name: CGColorSpace.sRGB)!,
                                    intent: .defaultIntent, options: nil),
          let c = rgb.components, c.count >= 3
    else { return nil }
    let r = Int((c[0] * 255).rounded()), g = Int((c[1] * 255).rounded()),
        b = Int((c[2] * 255).rounded())
    return String(format: "#%02X%02X%02X", r, g, b)
}

private func participant(_ p: EKParticipant) -> Participant {
    var email: String? = nil
    let url = p.url.absoluteString
    if url.lowercased().hasPrefix("mailto:") {
        email = String(url.dropFirst("mailto:".count))
    }
    let name = p.name?.trimmingCharacters(in: .whitespacesAndNewlines)
    return Participant(
        name: (name?.isEmpty ?? true) ? nil : name,
        email: email,
        status: p.participantStatus.rawValue,
        kind: p.participantType.rawValue,
        is_current_user: p.isCurrentUser
    )
}

private func toJSON<T: Encodable>(_ value: T) -> UnsafeMutablePointer<CChar>? {
    guard let data = try? JSONEncoder().encode(value),
          let text = String(data: data, encoding: .utf8)
    else { return nil }
    return strdup(text)
}

@_cdecl("handy_calendar_calendars_json")
public func handyCalendarCalendarsJSON() -> UnsafeMutablePointer<CChar>? {
    lock.lock()
    defer { lock.unlock() }
    let calendars = sharedStore.calendars(for: .event).map { cal in
        CalendarInfo(
            id: cal.calendarIdentifier,
            title: cal.title,
            source: cal.source?.title ?? "",
            color: hexColor(cal.cgColor)
        )
    }
    return toJSON(calendars)
}

/// Events overlapping [from, to] (Unix seconds), recurring ones expanded.
@_cdecl("handy_calendar_events_json")
public func handyCalendarEventsJSON(_ from: Double, _ to: Double) -> UnsafeMutablePointer<CChar>? {
    lock.lock()
    defer { lock.unlock() }
    let predicate = sharedStore.predicateForEvents(
        withStart: Date(timeIntervalSince1970: from),
        end: Date(timeIntervalSince1970: to),
        calendars: nil
    )
    let events = sharedStore.events(matching: predicate).compactMap { e -> EventInfo? in
        guard let start = e.startDate, let end = e.endDate else { return nil }
        var notes = e.notes
        if let n = notes, n.count > maxNotesLength {
            notes = String(n.prefix(maxNotesLength))
        }
        return EventInfo(
            series_id: e.eventIdentifier ?? e.calendarItemIdentifier,
            title: e.title ?? "",
            start_ms: Int64(start.timeIntervalSince1970 * 1000),
            end_ms: Int64(end.timeIntervalSince1970 * 1000),
            all_day: e.isAllDay,
            location: e.location,
            notes: notes,
            url: e.url?.absoluteString,
            calendar_id: e.calendar?.calendarIdentifier ?? "",
            calendar_title: e.calendar?.title ?? "",
            calendar_color: hexColor(e.calendar?.cgColor),
            status: e.status.rawValue,
            recurring: e.hasRecurrenceRules,
            organizer: e.organizer.map(participant),
            attendees: (e.attendees ?? []).map(participant)
        )
    }
    return toJSON(events)
}

@_cdecl("handy_calendar_free")
public func handyCalendarFree(_ pointer: UnsafeMutablePointer<CChar>?) {
    free(pointer)
}

// MARK: - Notifications

// The "meeting in 1 min — Record / Ignore" prompts. UNUserNotificationCenter
// only works from an app bundle (it throws when the binary runs on its own,
// as in `tauri dev`), so everything checks `notificationsAvailable` first and
// Rust falls back to showing the prompt in the window.

public typealias HandyNotificationCallback = @convention(c) (
    UnsafePointer<CChar>?, UnsafePointer<CChar>?
) -> Void

private var notificationCallback: HandyNotificationCallback?
private var notificationCategories: [String: UNNotificationCategory] = [:]

private func notificationsAvailable() -> Bool {
    return Bundle.main.bundleIdentifier != nil && Bundle.main.bundleURL.pathExtension == "app"
}

private final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate {
    // Handy is often the frontmost app (its window open): show banners anyway.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let action: String
        switch response.actionIdentifier {
        case UNNotificationDefaultActionIdentifier: action = "open"
        case UNNotificationDismissActionIdentifier: action = "dismiss"
        default: action = response.actionIdentifier
        }
        let id = response.notification.request.identifier
        if let callback = notificationCallback {
            action.withCString { a in id.withCString { i in callback(a, i) } }
        }
        completionHandler()
    }
}

private let notificationDelegate = NotificationDelegate()

/// Returns 1 when notifications can be used, 0 otherwise.
@_cdecl("handy_notifications_setup")
public func handyNotificationsSetup(_ callback: HandyNotificationCallback) -> Int32 {
    guard notificationsAvailable() else { return 0 }
    notificationCallback = callback
    UNUserNotificationCenter.current().delegate = notificationDelegate
    return 1
}

private func notificationStatusCode(_ status: UNAuthorizationStatus) -> Int32 {
    return Int32(status.rawValue)
}

/// UNAuthorizationStatus raw value, or -1 when unavailable.
@_cdecl("handy_notifications_status")
public func handyNotificationsStatus() -> Int32 {
    guard notificationsAvailable() else { return -1 }
    let semaphore = DispatchSemaphore(value: 0)
    var code: Int32 = 0
    UNUserNotificationCenter.current().getNotificationSettings { settings in
        code = notificationStatusCode(settings.authorizationStatus)
        semaphore.signal()
    }
    semaphore.wait()
    return code
}

/// Shows the system prompt (only the first time) and blocks until the user
/// answers. Must not be called on the main thread.
@_cdecl("handy_notifications_request")
public func handyNotificationsRequest() -> Int32 {
    guard notificationsAvailable() else { return -1 }
    let semaphore = DispatchSemaphore(value: 0)
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, error in
        if let error = error {
            NSLog("Handy notification authorization failed: \(error.localizedDescription)")
        }
        semaphore.signal()
    }
    semaphore.wait()
    return handyNotificationsStatus()
}

private struct ActionInfo: Codable {
    let id: String
    let title: String
    let destructive: Bool
}

private func string(_ pointer: UnsafePointer<CChar>?) -> String {
    guard let pointer = pointer else { return "" }
    return String(cString: pointer)
}

/// Posts (or replaces, same id) a notification whose buttons are described
/// by `actionsJSON`. Returns 1 when it was handed to the system.
@_cdecl("handy_notification_post")
public func handyNotificationPost(
    _ id: UnsafePointer<CChar>?,
    _ title: UnsafePointer<CChar>?,
    _ body: UnsafePointer<CChar>?,
    _ category: UnsafePointer<CChar>?,
    _ actionsJSON: UnsafePointer<CChar>?
) -> Int32 {
    guard notificationsAvailable() else { return 0 }
    let identifier = string(id)
    let categoryId = string(category)
    let actions = (try? JSONDecoder().decode(
        [ActionInfo].self, from: Data(string(actionsJSON).utf8))) ?? []

    let center = UNUserNotificationCenter.current()
    // Button titles follow the app language, so the category is rebuilt each
    // time; the set replaces every category at once.
    lock.lock()
    notificationCategories[categoryId] = UNNotificationCategory(
        identifier: categoryId,
        actions: actions.map {
            UNNotificationAction(
                identifier: $0.id, title: $0.title,
                options: $0.destructive ? [.destructive] : [])
        },
        intentIdentifiers: [],
        options: []
    )
    center.setNotificationCategories(Set(notificationCategories.values))
    lock.unlock()

    let content = UNMutableNotificationContent()
    content.title = string(title)
    content.body = string(body)
    content.categoryIdentifier = categoryId
    content.sound = .default
    let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
    center.add(request) { error in
        if let error = error {
            NSLog("Handy notification failed: \(error.localizedDescription)")
        }
    }
    return 1
}

@_cdecl("handy_notification_remove")
public func handyNotificationRemove(_ id: UnsafePointer<CChar>?) {
    guard notificationsAvailable() else { return }
    let identifier = string(id)
    let center = UNUserNotificationCenter.current()
    center.removeDeliveredNotifications(withIdentifiers: [identifier])
    center.removePendingNotificationRequests(withIdentifiers: [identifier])
}
