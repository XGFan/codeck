//! System notifications through `UNUserNotificationCenter`: sending one, the click on one, and taking delivered
//! ones out of Notification Center.
//!
//! Only the bundled app has them: `pnpm tauri dev` runs a bare executable, which the notification center does not
//! take (asking for it there raises), so there nothing is sent.

use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;
use specta::Type;

/// A notification to show.
#[derive(Deserialize, Type, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// Names it: a later one with the same `id` takes its place, and `remove` finds it by this.
    pub id: String,
    pub title: String,
    pub body: String,
    /// Reported when it is clicked (a `codeck://` link).
    pub url: String,
    /// Notification Center keeps the notifications of one thread together.
    pub thread: Option<String>,
    /// Time-sensitive: shown during a Focus that lets such notifications through. It takes the entitlement
    /// `com.apple.developer.usernotifications.time-sensitive`; an app signed without it has it shown as a plain one.
    pub time_sensitive: bool,
}

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
