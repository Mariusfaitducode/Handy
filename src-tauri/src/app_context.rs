//! Which app (and, for browsers, which site) the user is dictating into.
//!
//! macOS only. The app comes from `NSWorkspace.frontmostApplication` (no
//! permission needed). For browsers, the focused tab is read through the
//! Accessibility API, which Handy already needs to paste: first the web
//! area's URL, falling back to the window title ("Inbox - Gmail"). Nothing
//! here prompts for a new permission; without Accessibility access the
//! browser itself is used as the context.

// Detection is macOS-only; elsewhere the helpers are only exercised by tests.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde::Serialize;

/// Prefix of keys that identify a website rather than an app.
pub const WEB_KEY_PREFIX: &str = "web:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppContext {
    /// Stable key used for style rules: the bundle id for apps, or
    /// `web:<site>` when a browser tab was recognised.
    pub app_key: String,
    /// Human-readable name ("Mail", "Gmail", "notion.so").
    pub app_name: String,
    /// Bundle id of the frontmost app (the browser for web contexts).
    pub bundle_id: String,
}

const BROWSERS: &[&str] = &[
    "com.apple.Safari",
    "com.apple.SafariTechnologyPreview",
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.canary",
    "company.thebrowser.Browser",
    "company.thebrowser.dia",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "org.mozilla.firefox",
    "org.mozilla.firefoxdeveloperedition",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
    "app.zen-browser.zen",
    "com.kagi.kagimacOS",
    "org.chromium.Chromium",
];

pub fn is_browser(bundle_id: &str) -> bool {
    BROWSERS.contains(&bundle_id)
}

/// Known sites: (site id, display name, host suffixes, title keywords).
/// Title keywords are matched against whole " - "/" | " separated segments of
/// the tab title, which is how these web apps name their tabs.
const KNOWN_SITES: &[(&str, &str, &[&str], &[&str])] = &[
    ("gmail", "Gmail", &["mail.google.com"], &["Gmail"]),
    (
        "outlook",
        "Outlook",
        &[
            "outlook.live.com",
            "outlook.office.com",
            "outlook.office365.com",
        ],
        &["Outlook"],
    ),
    (
        "protonmail",
        "Proton Mail",
        &["mail.proton.me"],
        &["Proton Mail"],
    ),
    ("slack", "Slack", &["slack.com"], &["Slack"]),
    (
        "whatsapp",
        "WhatsApp",
        &["web.whatsapp.com", "whatsapp.com"],
        &["WhatsApp"],
    ),
    ("messenger", "Messenger", &["messenger.com"], &["Messenger"]),
    ("discord", "Discord", &["discord.com"], &["Discord"]),
    (
        "teams",
        "Microsoft Teams",
        &["teams.microsoft.com", "teams.live.com"],
        &["Microsoft Teams"],
    ),
    ("telegram", "Telegram", &["web.telegram.org"], &["Telegram"]),
    ("claude", "Claude", &["claude.ai"], &["Claude"]),
    (
        "chatgpt",
        "ChatGPT",
        &["chatgpt.com", "chat.openai.com"],
        &["ChatGPT"],
    ),
    ("github", "GitHub", &["github.com"], &["GitHub"]),
];

fn host_matches(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

/// Extracts the lowercase host of an http(s) URL, without `www.` or port.
pub fn host_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    let host = host.trim().to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host)
}

/// Maps a browser tab URL to a (key, name). Unknown sites keep their host so
/// users can still give them a style.
pub fn site_from_url(url: &str) -> Option<(String, String)> {
    let host = host_from_url(url)?;
    for (id, name, hosts, _) in KNOWN_SITES {
        if hosts.iter().any(|h| host_matches(&host, h)) {
            return Some((format!("{WEB_KEY_PREFIX}{id}"), name.to_string()));
        }
    }
    Some((format!("{WEB_KEY_PREFIX}{host}"), host))
}

/// Recognises a known web app from its tab title. Only known sites: an
/// arbitrary page title is not a stable key.
pub fn site_from_title(title: &str) -> Option<(String, String)> {
    let segments: Vec<String> = title
        .split([' ', '\u{a0}'])
        .collect::<Vec<_>>()
        .join(" ")
        .split(" - ")
        .flat_map(|s| s.split(" – "))
        .flat_map(|s| s.split(" | "))
        .map(|s| {
            // "(3) WhatsApp" → "WhatsApp"
            let s = s.trim();
            let s = if s.starts_with('(') {
                s.split_once(") ").map(|(_, r)| r).unwrap_or(s)
            } else {
                s
            };
            s.trim().to_lowercase()
        })
        .collect();
    for (id, name, _, keywords) in KNOWN_SITES {
        if keywords
            .iter()
            .any(|k| segments.iter().any(|s| s == &k.to_lowercase()))
        {
            return Some((format!("{WEB_KEY_PREFIX}{id}"), name.to_string()));
        }
    }
    None
}

/// Builds the context for an app, refining it with the tab's URL or title
/// when the app is a browser.
pub fn build_context(
    bundle_id: &str,
    app_name: &str,
    url: Option<&str>,
    title: Option<&str>,
) -> AppContext {
    if is_browser(bundle_id) {
        let site = url
            .and_then(site_from_url)
            .or_else(|| title.and_then(site_from_title));
        if let Some((app_key, app_name)) = site {
            return AppContext {
                app_key,
                app_name,
                bundle_id: bundle_id.to_string(),
            };
        }
    }
    AppContext {
        app_key: bundle_id.to_string(),
        app_name: app_name.to_string(),
        bundle_id: bundle_id.to_string(),
    }
}

/// True for Handy's own bundle (release or dev builds).
fn is_handy(bundle_id: &str) -> bool {
    bundle_id == "com.pais.handy" || bundle_id.starts_with("com.pais.handy.")
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
    use std::ffi::{c_char, c_void, CStr};

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type AXUIElementRef = *const c_void;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const AX_SUCCESS: i32 = 0;
    /// Upper bound for each Accessibility call, so a hung app can't stall the
    /// dictation. Typical calls take well under a millisecond.
    const AX_TIMEOUT_SECONDS: f32 = 0.15;
    const MAX_PARENT_DEPTH: usize = 40;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFURLGetTypeID() -> usize;
        fn CFURLGetString(url: CFTypeRef) -> CFStringRef;
        fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            num_bytes: isize,
            encoding: u32,
            is_external: bool,
        ) -> CFStringRef;
        fn CFStringGetLength(s: CFStringRef) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(
            s: CFStringRef,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> bool;
    }

    /// Owned CoreFoundation reference (Create/Copy rule), released on drop.
    struct Cf(CFTypeRef);
    impl Drop for Cf {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: only constructed from Create/Copy results we own.
                unsafe { CFRelease(self.0) };
            }
        }
    }

    fn cf_string(s: &str) -> Cf {
        // SAFETY: bytes/len describe a valid UTF-8 buffer for the call.
        Cf(unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                s.as_ptr(),
                s.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
                false,
            )
        })
    }

    /// Copies a borrowed CFString into a Rust string.
    fn string_from_cf(s: CFStringRef) -> Option<String> {
        if s.is_null() {
            return None;
        }
        // SAFETY: `s` is a valid CFString for the duration of these calls and
        // the buffer is sized with CFStringGetMaximumSizeForEncoding (+NUL).
        unsafe {
            if CFGetTypeID(s) != CFStringGetTypeID() {
                return None;
            }
            let len = CFStringGetLength(s);
            let cap = CFStringGetMaximumSizeForEncoding(len, K_CF_STRING_ENCODING_UTF8) + 1;
            let mut buf = vec![0 as c_char; cap.max(1) as usize];
            if !CFStringGetCString(s, buf.as_mut_ptr(), cap, K_CF_STRING_ENCODING_UTF8) {
                return None;
            }
            Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }

    fn copy_attr(element: AXUIElementRef, name: &str) -> Option<Cf> {
        if element.is_null() {
            return None;
        }
        let attr = cf_string(name);
        let mut value: CFTypeRef = std::ptr::null();
        // SAFETY: element is a live AXUIElement, attr a valid CFString, and
        // value receives a +1 reference on success (Copy rule).
        let err = unsafe { AXUIElementCopyAttributeValue(element, attr.0, &mut value) };
        if err != AX_SUCCESS || value.is_null() {
            return None;
        }
        Some(Cf(value))
    }

    fn attr_string(element: AXUIElementRef, name: &str) -> Option<String> {
        let value = copy_attr(element, name)?;
        string_from_cf(value.0)
    }

    fn attr_url(element: AXUIElementRef, name: &str) -> Option<String> {
        let value = copy_attr(element, name)?;
        // SAFETY: type-checked before treating it as a CFURL; CFURLGetString
        // returns a borrowed string owned by the URL, which outlives the read.
        unsafe {
            if CFGetTypeID(value.0) == CFURLGetTypeID() {
                return string_from_cf(CFURLGetString(value.0));
            }
        }
        string_from_cf(value.0)
    }

    /// URL and title of the focused tab of the browser with `pid`.
    fn browser_tab(pid: i32) -> (Option<String>, Option<String>) {
        // SAFETY: plain query of the process's own trust state.
        if !unsafe { AXIsProcessTrusted() } {
            return (None, None);
        }
        // SAFETY: returns a +1 AXUIElement (or null) for this pid.
        let app = Cf(unsafe { AXUIElementCreateApplication(pid) });
        if app.0.is_null() {
            return (None, None);
        }
        // SAFETY: app is a live AXUIElement.
        unsafe { AXUIElementSetMessagingTimeout(app.0, AX_TIMEOUT_SECONDS) };

        // Walk up from the focused element to the web area that holds it:
        // its AXURL is the URL of the page being typed into.
        let mut url = None;
        if let Some(mut current) = copy_attr(app.0, "AXFocusedUIElement") {
            for _ in 0..MAX_PARENT_DEPTH {
                if attr_string(current.0, "AXRole").as_deref() == Some("AXWebArea") {
                    url = attr_url(current.0, "AXURL");
                    break;
                }
                match copy_attr(current.0, "AXParent") {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
        }

        let window = copy_attr(app.0, "AXFocusedWindow");
        let title = window
            .as_ref()
            .and_then(|w| attr_string(w.0, "AXTitle"))
            .filter(|t| !t.trim().is_empty());
        if url.is_none() {
            url = window
                .as_ref()
                .and_then(|w| attr_url(w.0, "AXDocument"))
                .filter(|u| u.starts_with("http"));
        }
        (url, title)
    }

    pub fn detect_frontmost() -> Option<AppContext> {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication()?;
        let bundle_id = app.bundleIdentifier()?.to_string();
        if is_handy(&bundle_id) || app.processIdentifier() == std::process::id() as i32 {
            return None;
        }
        let name = app
            .localizedName()
            .map(|n| n.to_string())
            .unwrap_or_else(|| bundle_id.clone());

        let (url, title) = if is_browser(&bundle_id) {
            browser_tab(app.processIdentifier())
        } else {
            (None, None)
        };
        Some(build_context(
            &bundle_id,
            &name,
            url.as_deref(),
            title.as_deref(),
        ))
    }

    pub fn running_apps() -> Vec<(String, String)> {
        let workspace = NSWorkspace::sharedWorkspace();
        let mut apps: Vec<(String, String)> = workspace
            .runningApplications()
            .iter()
            .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
            .filter_map(|a| {
                let bundle_id = a.bundleIdentifier()?.to_string();
                if is_handy(&bundle_id) || bundle_id == "com.apple.finder" {
                    return None;
                }
                let name = a.localizedName()?.to_string();
                Some((bundle_id, name))
            })
            .collect();
        apps.sort_by_key(|(_, name)| name.to_lowercase());
        apps.dedup_by(|a, b| a.0 == b.0);
        apps
    }
}

/// The app that will receive the dictated text, or `None` when it can't be
/// told (another OS, Handy itself focused, no bundle id).
pub fn detect_frontmost() -> Option<AppContext> {
    #[cfg(target_os = "macos")]
    {
        macos::detect_frontmost()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Open regular apps as (bundle id, name), sorted by name.
pub fn running_apps() -> Vec<(String, String)> {
    #[cfg(target_os = "macos")]
    {
        macos::running_apps()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_is_extracted_from_urls() {
        assert_eq!(
            host_from_url("https://mail.google.com/mail/u/0/#inbox").as_deref(),
            Some("mail.google.com")
        );
        assert_eq!(
            host_from_url("https://www.Notion.so:443/page?x=1").as_deref(),
            Some("notion.so")
        );
        assert_eq!(host_from_url("about:blank"), None);
        assert_eq!(host_from_url("http://localhost:3000/"), None);
    }

    #[test]
    fn known_sites_are_recognised_from_urls() {
        assert_eq!(
            site_from_url("https://mail.google.com/mail/u/0/"),
            Some(("web:gmail".into(), "Gmail".into()))
        );
        assert_eq!(
            site_from_url("https://acme.slack.com/archives/C1"),
            Some(("web:slack".into(), "Slack".into()))
        );
        assert_eq!(
            site_from_url("https://www.notion.so/x"),
            Some(("web:notion.so".into(), "notion.so".into()))
        );
        // Suffix match must be on a label boundary.
        assert_eq!(
            site_from_url("https://notslack.com/"),
            Some(("web:notslack.com".into(), "notslack.com".into()))
        );
    }

    #[test]
    fn known_sites_are_recognised_from_titles() {
        assert_eq!(
            site_from_title("Inbox (12) - marius@gmail.com - Gmail").map(|s| s.0),
            Some("web:gmail".into())
        );
        assert_eq!(
            site_from_title("(3) WhatsApp").map(|s| s.0),
            Some("web:whatsapp".into())
        );
        assert_eq!(
            site_from_title("general (Channel) - Acme - Slack").map(|s| s.0),
            Some("web:slack".into())
        );
        assert_eq!(
            site_from_title("Claude").map(|s| s.0),
            Some("web:claude".into())
        );
        // A word inside a longer segment is not a match.
        assert_eq!(site_from_title("How to use Slack well"), None);
        assert_eq!(site_from_title("Hacker News"), None);
    }

    #[test]
    fn browser_context_prefers_url_then_title_then_browser() {
        let c = build_context(
            "com.google.Chrome",
            "Google Chrome",
            Some("https://claude.ai/new"),
            Some("Inbox - Gmail"),
        );
        assert_eq!(c.app_key, "web:claude");
        assert_eq!(c.bundle_id, "com.google.Chrome");

        let c = build_context("com.apple.Safari", "Safari", None, Some("Inbox - Gmail"));
        assert_eq!(c.app_key, "web:gmail");

        let c = build_context("com.apple.Safari", "Safari", None, Some("Hacker News"));
        assert_eq!(c.app_key, "com.apple.Safari");
        assert_eq!(c.app_name, "Safari");
    }

    #[test]
    fn non_browsers_ignore_titles() {
        let c = build_context("com.apple.Notes", "Notes", None, Some("Gmail"));
        assert_eq!(c.app_key, "com.apple.Notes");
    }

    /// Manual smoke test against the real desktop:
    /// `cargo test print_frontmost_app -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_frontmost_app() {
        println!("frontmost: {:?}", detect_frontmost());
        println!("running: {:?}", running_apps());
    }

    #[test]
    fn handy_is_never_a_target() {
        assert!(is_handy("com.pais.handy"));
        assert!(!is_handy("com.apple.mail"));
    }
}
