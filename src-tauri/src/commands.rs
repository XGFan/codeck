//! IPC commands: one per entry of the Host's method table (`codeck_host::for_each_method!`), each
//! calling the Host method of the same name (async; the Host runs blocking work on the blocking pool).

use std::sync::Arc;

use codeck_core::model::AppError;
use codeck_host::Host;
use tauri::State;

type CmdResult<T> = Result<T, AppError>;

macro_rules! tauri_commands {
    ($( $(#[$meta:meta])* $scope:ident fn $name:ident ( $($p:ident : $t:ty),* ) -> $ret:ty; )*) => {
        $(
            $(#[$meta])*
            #[tauri::command]
            #[specta::specta]
            pub async fn $name(host: State<'_, Arc<Host>>, $($p: $t),*) -> CmdResult<$ret> {
                host.$name($($p),*).await
            }
        )*

        /// Every command, in the table's order, then the shell's own.
        pub fn collect() -> tauri_specta::Commands<tauri::Wry> {
            tauri_specta::collect_commands![$($name,)* remove_notifications]
        }
    };
}

codeck_host::for_each_method!(tauri_commands);

/// Takes this app's delivered notifications with exactly this title and body out of Notification Center (the
/// notification plugin cannot on macOS). Not in the Host's table: it is the window's, like the Dock badge.
#[tauri::command]
#[specta::specta]
pub async fn remove_notifications(
    app: tauri::AppHandle,
    title: String,
    body: String,
) -> CmdResult<()> {
    #[cfg(target_os = "macos")]
    app.run_on_main_thread(move || macos::remove_delivered(&title, &body))
        .map_err(|e| AppError {
            code: codeck_core::model::ErrorCode::Internal,
            message: format!("删不了通知：{e}"),
        })?;
    #[cfg(not(target_os = "macos"))]
    let _ = (app, title, body);
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSArray, NSString};

    /// `NSUserNotificationCenter`, which the notification plugin (`mac-notification-sys`) delivers through.
    pub fn remove_delivered(title: &str, body: &str) {
        // SAFETY: plain messages to the shared notification center on the main thread; every object used is
        // retained for the time it is used.
        unsafe {
            let center: Option<Retained<AnyObject>> = msg_send![
                class!(NSUserNotificationCenter),
                defaultUserNotificationCenter
            ];
            let Some(center) = center else { return };
            let delivered: Option<Retained<NSArray<AnyObject>>> =
                msg_send![&*center, deliveredNotifications];
            for n in delivered.iter().flat_map(|a| a.iter()) {
                let t: Option<Retained<NSString>> = msg_send![&*n, title];
                let b: Option<Retained<NSString>> = msg_send![&*n, informativeText];
                if t.is_some_and(|t| t.to_string() == title)
                    && b.is_some_and(|b| b.to_string() == body)
                {
                    let _: () = msg_send![&*center, removeDeliveredNotification: &*n];
                }
            }
        }
    }
}
