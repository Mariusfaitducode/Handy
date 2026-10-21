//! The EventKit and notification side, implemented in
//! `swift/calendar_bridge.swift`. Other platforms report everything
//! unsupported.

use serde::Deserialize;

use super::detect::RawEvent;
use super::{CalendarAccess, NotificationAccess};

/// A calendar as the bridge reports it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RawCalendar {
    pub id: String,
    pub title: String,
    pub source: String,
    pub color: Option<String>,
}

/// A notification button.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NotificationAction<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub destructive: bool,
}

pub struct Notification<'a> {
    /// Posting again with the same id replaces the notification.
    pub id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    /// Groups notifications that share the same buttons.
    pub category: &'a str,
    pub actions: &'a [NotificationAction<'a>],
}

/// `action` is a button id, "open" (the notification itself was clicked) or
/// "dismiss"; `id` is the notification's id.
pub type ResponseHandler = fn(action: &str, id: &str);

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn access_from_code(code: i32) -> CalendarAccess {
    // EKAuthorizationStatus: 0 not determined, 1 restricted, 2 denied,
    // 3 full access, 4 write only (useless to read).
    match code {
        0 => CalendarAccess::NotDetermined,
        3 => CalendarAccess::Granted,
        _ => CalendarAccess::Denied,
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn notification_access_from_code(code: i32) -> NotificationAccess {
    // UNAuthorizationStatus: 0 not determined, 1 denied, 2 authorized,
    // 3 provisional, 4 ephemeral; -1 when notifications are unavailable.
    match code {
        0 => NotificationAccess::NotDetermined,
        1 => NotificationAccess::Denied,
        2..=4 => NotificationAccess::Granted,
        _ => NotificationAccess::Unavailable,
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;
    use std::sync::OnceLock;

    type Callback = extern "C" fn(*const c_char, *const c_char);

    extern "C" {
        fn handy_calendar_authorization() -> i32;
        fn handy_calendar_request_access() -> i32;
        fn handy_calendar_calendars_json() -> *mut c_char;
        fn handy_calendar_events_json(from: f64, to: f64) -> *mut c_char;
        fn handy_calendar_free(pointer: *mut c_char);
        fn handy_notifications_setup(callback: Callback) -> i32;
        fn handy_notifications_status() -> i32;
        fn handy_notifications_request() -> i32;
        fn handy_notification_post(
            id: *const c_char,
            title: *const c_char,
            body: *const c_char,
            category: *const c_char,
            actions_json: *const c_char,
        ) -> i32;
        fn handy_notification_remove(id: *const c_char);
    }

    static HANDLER: OnceLock<ResponseHandler> = OnceLock::new();

    extern "C" fn on_response(action: *const c_char, id: *const c_char) {
        let read = |p: *const c_char| {
            if p.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
            }
        };
        if let Some(handler) = HANDLER.get() {
            handler(&read(action), &read(id));
        }
    }

    fn take_json(pointer: *mut c_char) -> Option<String> {
        if pointer.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned();
        unsafe { handy_calendar_free(pointer) };
        Some(text)
    }

    fn c(text: &str) -> CString {
        CString::new(text.replace('\0', "")).unwrap_or_default()
    }

    pub fn access() -> CalendarAccess {
        access_from_code(unsafe { handy_calendar_authorization() })
    }

    pub fn request_access() -> CalendarAccess {
        access_from_code(unsafe { handy_calendar_request_access() })
    }

    pub fn calendars() -> Result<Vec<RawCalendar>, String> {
        let json = take_json(unsafe { handy_calendar_calendars_json() })
            .ok_or("calendar bridge returned nothing")?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }

    pub fn events(from_ms: i64, to_ms: i64) -> Result<Vec<RawEvent>, String> {
        let json = take_json(unsafe {
            handy_calendar_events_json(from_ms as f64 / 1000.0, to_ms as f64 / 1000.0)
        })
        .ok_or("calendar bridge returned nothing")?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }

    pub fn setup_notifications(handler: ResponseHandler) -> bool {
        let _ = HANDLER.set(handler);
        unsafe { handy_notifications_setup(on_response) == 1 }
    }

    pub fn notification_access() -> NotificationAccess {
        notification_access_from_code(unsafe { handy_notifications_status() })
    }

    pub fn request_notifications() -> NotificationAccess {
        notification_access_from_code(unsafe { handy_notifications_request() })
    }

    pub fn post(n: &Notification<'_>) -> bool {
        let actions = serde_json::to_string(n.actions).unwrap_or_else(|_| "[]".into());
        let (id, title, body, category, actions) =
            (c(n.id), c(n.title), c(n.body), c(n.category), c(&actions));
        unsafe {
            handy_notification_post(
                id.as_ptr(),
                title.as_ptr(),
                body.as_ptr(),
                category.as_ptr(),
                actions.as_ptr(),
            ) == 1
        }
    }

    pub fn remove(id: &str) {
        let id = c(id);
        unsafe { handy_notification_remove(id.as_ptr()) }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn access() -> CalendarAccess {
        CalendarAccess::Unsupported
    }
    pub fn request_access() -> CalendarAccess {
        CalendarAccess::Unsupported
    }
    pub fn calendars() -> Result<Vec<RawCalendar>, String> {
        Err("unsupported".into())
    }
    pub fn events(_from_ms: i64, _to_ms: i64) -> Result<Vec<RawEvent>, String> {
        Err("unsupported".into())
    }
    pub fn setup_notifications(_handler: ResponseHandler) -> bool {
        false
    }
    pub fn notification_access() -> NotificationAccess {
        NotificationAccess::Unavailable
    }
    pub fn request_notifications() -> NotificationAccess {
        NotificationAccess::Unavailable
    }
    pub fn post(_n: &Notification<'_>) -> bool {
        false
    }
    pub fn remove(_id: &str) {}
}

pub use imp::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_map() {
        assert_eq!(access_from_code(0), CalendarAccess::NotDetermined);
        assert_eq!(access_from_code(3), CalendarAccess::Granted);
        assert_eq!(
            access_from_code(4),
            CalendarAccess::Denied,
            "write-only can't read"
        );
        assert_eq!(access_from_code(1), CalendarAccess::Denied);
        assert_eq!(
            notification_access_from_code(3),
            NotificationAccess::Granted
        );
        assert_eq!(
            notification_access_from_code(-1),
            NotificationAccess::Unavailable
        );
    }
}
