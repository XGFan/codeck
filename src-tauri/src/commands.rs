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

        /// Every command, in the table's order.
        pub fn collect() -> tauri_specta::Commands<tauri::Wry> {
            tauri_specta::collect_commands![$($name),*]
        }
    };
}

codeck_host::for_each_method!(tauri_commands);
