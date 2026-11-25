//! IPC commands: one per entry of the Host's method table (`codeck_host::for_each_method!`), each
//! calling the Host method of the same name (async; the Host runs blocking work on the blocking pool).

use std::sync::Arc;

use codeck_core::model::AppError;
use codeck_host::Host;
use tauri::State;

use crate::notify::Notice;

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
            tauri_specta::collect_commands![$($name,)* notify, remove_notifications]
        }
    };
}

codeck_host::for_each_method!(tauri_commands);

/// Shows a system notification, in place of the one with its `id` when that is still there. Not in the Host's
/// table: it is the window's, like the Dock badge.
#[tauri::command]
#[specta::specta]
pub async fn notify(notice: Notice) {
    crate::notify::send(notice);
}

/// Takes the notifications with these `id`s (as given to `notify`) out of Notification Center.
#[tauri::command]
#[specta::specta]
pub async fn remove_notifications(ids: Vec<String>) {
    crate::notify::remove(ids);
}
