//! System notifications through `UNUserNotificationCenter`: sending one, the click on one, and taking delivered
//! ones out of Notification Center.
//!
//! Only the bundled app has them: `pnpm tauri dev` runs a bare executable, which the notification center does not
//! take (asking for it there raises), so there nothing is sent.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use codeck_host::attention::{Notice, Notifier};

/// Someone uses the Mac when the keyboard or mouse was used this lately (and the screen is not locked); otherwise the
/// Host pushes to the iPhone instead (ADR-0019).
const PRESENT_WITHIN_SECS: f64 = 120.0;

/// The executable sits in an app bundle (`<name>.app/Contents/MacOS/<executable>`).
fn bundled(exe: &Path) -> bool {
    let mut dirs = exe.ancestors().skip(1);
    dirs.next().is_some_and(|d| d.ends_with("Contents/MacOS"))
        && dirs
            .nth(1)
            .is_some_and(|d| d.extension().is_some_and(|e| e == "app"))
}

/// This process can use the notification center.
fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        cfg!(target_os = "macos") && std::env::current_exe().is_ok_and(|exe| bundled(&exe))
    })
}

/// Asks for the permission (the system asks the user once) and has `on_click` called with the `url` of each
/// notification that is clicked. Call once, at startup.
pub fn init(on_click: impl Fn(String) + Send + Sync + 'static) {
    if !available() {
        tracing::info!("not an app bundle: no system notifications");
        return;
    }
    #[cfg(target_os = "macos")]
    macos::init(Box::new(on_click));
    #[cfg(not(target_os = "macos"))]
    let _ = on_click;
}

/// Shows `notice`, in place of the one with its `id` when that is still there.
pub fn send(notice: Notice) {
    if !available() {
        return;
    }
    #[cfg(target_os = "macos")]
    macos::send(notice);
    #[cfg(not(target_os = "macos"))]
    let _ = notice;
}

/// Takes the notifications with these `id`s out of Notification Center.
pub fn remove(ids: Vec<String>) {
    if !available() || ids.is_empty() {
        return;
    }
    #[cfg(target_os = "macos")]
    macos::remove(ids);
}

/// Someone uses the Mac now: input within [`PRESENT_WITHIN_SECS`] and the screen not locked.
fn present() -> bool {
    #[cfg(target_os = "macos")]
    return macos::idle_secs() < PRESENT_WITHIN_SECS && !macos::screen_locked();
    #[cfg(not(target_os = "macos"))]
    true
}

/// The Mac's notifications for the Host. Time-sensitive ones take the entitlement
/// `com.apple.developer.usernotifications.time-sensitive`; an app signed without it has them shown as plain ones.
struct MacNotifier;

impl Notifier for MacNotifier {
    fn present(&self) -> bool {
        present()
    }

    fn send(&self, notice: Notice) {
        send(notice)
    }

    fn remove(&self, ids: Vec<String>) {
        remove(ids)
    }
}

/// The Host's way to the Mac's notifications; `None` outside an app bundle.
pub fn notifier() -> Option<Arc<dyn Notifier>> {
    available().then(|| Arc::new(MacNotifier) as Arc<dyn Notifier>)
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::{DynBlock, RcBlock};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
    use objc2_foundation::{NSArray, NSDictionary, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationDefaultActionIdentifier, UNNotificationInterruptionLevel,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::Notice;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(source: i32, event_type: u32) -> f64;
        fn CGSessionCopyCurrentDictionary() -> *mut AnyObject;
    }
    /// `kCGEventSourceStateCombinedSessionState`.
    const COMBINED_SESSION_STATE: i32 = 0;
    /// `kCGAnyInputEventType`.
    const ANY_INPUT: u32 = u32::MAX;

    /// Seconds since the last keyboard or mouse input in this login session.
    pub fn idle_secs() -> f64 {
        // SAFETY: a plain C function taking two integers.
        unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT) }
    }

    /// The login session's screen is locked (`CGSSessionScreenIsLocked`, present only while it is).
    pub fn screen_locked() -> bool {
        // SAFETY: returns a CFDictionary we own (or null); CFDictionary is toll-free bridged to NSDictionary, so
        // `Retained` takes over that reference and releases it.
        let Some(dict) = (unsafe { Retained::from_raw(CGSessionCopyCurrentDictionary()) }) else {
            return false;
        };
        // SAFETY: `objectForKey:` takes any object and returns an object or nil.
        let locked: Option<Retained<AnyObject>> = unsafe {
            msg_send![&*dict, objectForKey: &*NSString::from_str("CGSSessionScreenIsLocked")]
        };
        // SAFETY: its value is a CFBoolean, bridged to NSNumber, which answers `boolValue`.
        locked.is_some_and(|v| unsafe { msg_send![&*v, boolValue] })
    }

    /// Where a notification's `userInfo` keeps its link.
    const URL: &str = "url";

    type OnClick = Box<dyn Fn(String) + Send + Sync>;

    define_class!(
        // SAFETY: `NSObject` has no subclassing requirements, and `Delegate` does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[name = "CoDeckNotificationDelegate"]
        #[ivars = OnClick]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// One due while the app is in front is shown all the same: it is about another Session than the one
            /// being looked at.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                done: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                done.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                done: &DynBlock<dyn Fn()>,
            ) {
                // SAFETY: a constant of the framework.
                let click = unsafe { UNNotificationDefaultActionIdentifier };
                if &*response.actionIdentifier() == click {
                    let info = response.notification().request().content().userInfo();
                    // SAFETY: `objectForKey:` takes any object and returns an object or nil.
                    let url: Option<Retained<AnyObject>> =
                        unsafe { msg_send![&*info, objectForKey: &*NSString::from_str(URL)] };
                    if let Some(url) = url.and_then(|u| u.downcast::<NSString>().ok()) {
                        (self.ivars())(url.to_string());
                    }
                }
                done.call(());
            }
        }
    );

    pub fn init(on_click: OnClick) {
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::alloc().set_ivars(on_click);
        // SAFETY: `NSObject`'s `init`, on a newly allocated object whose ivars are set.
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        // The center holds its delegate weakly, and this one is for as long as the app runs.
        std::mem::forget(delegate);
        // The badge too: once the app has notification settings, the Dock badge follows them.
        let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Badge;
        let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
            if !granted.as_bool() {
                tracing::warn!("system notifications are not allowed: {}", describe(error));
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(options, &done);
    }

    pub fn send(n: Notice) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&n.title));
        content.setBody(&NSString::from_str(&n.body));
        if let Some(thread) = &n.thread {
            content.setThreadIdentifier(&NSString::from_str(thread));
        }
        if n.time_sensitive {
            content.setInterruptionLevel(UNNotificationInterruptionLevel::TimeSensitive);
        }
        let info = NSDictionary::from_slices(
            &[&*NSString::from_str(URL)],
            &[&*NSString::from_str(&n.url)],
        );
        // SAFETY: a dictionary of strings is a property list, which is what `userInfo` takes.
        let _: () = unsafe { msg_send![&*content, setUserInfo: &*info] };
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&n.id),
            &content,
            None,
        );
        let id = n.id;
        let done = RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() {
                tracing::warn!("notification {id} was not sent: {}", describe(error));
            }
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .addNotificationRequest_withCompletionHandler(&request, Some(&done));
    }

    pub fn remove(ids: Vec<String>) {
        let ids: Vec<_> = ids.iter().map(|id| NSString::from_str(id)).collect();
        UNUserNotificationCenter::currentNotificationCenter()
            .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&ids));
    }

    fn describe(error: *mut NSError) -> String {
        // SAFETY: the framework hands its completion handlers a valid error or nil.
        match unsafe { error.as_ref() } {
            Some(e) => e.localizedDescription().to_string(),
            None => "no reason given".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_executable_inside_an_app_bundle_is_bundled() {
        assert!(bundled(Path::new(
            "/Applications/CoDeck.app/Contents/MacOS/codeck"
        )));
        assert!(!bundled(Path::new("/repo/target/debug/codeck")));
        assert!(!bundled(Path::new("/repo/Contents/MacOS/codeck")));
        assert!(!bundled(Path::new("/x.app/Contents/Resources/codeck")));
    }
}
