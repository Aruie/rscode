// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod commands;
mod doc;
mod fuzzy;
mod lang;
mod remote;
#[cfg(feature = "screenshot")]
mod shot;
mod ssh_config;
mod state;
mod text_ops;
mod tree;

use slint::ComponentHandle;
use state::{with_state, State, STATE};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(feature = "screenshot")]
    {
        let a: Vec<String> = std::env::args().collect();
        if a.get(1).map(String::as_str) == Some("--screenshot") {
            shot::run(&a[2], &a[3]);
            return Ok(());
        }
    }
    // The software renderer keeps memory low (no GPU context, no webview).
    if std::env::var_os("SLINT_BACKEND").is_none() {
        // SAFETY: set before any other thread is started.
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-software") };
    }
    let app = App::new()?;
    STATE.with(|s| *s.borrow_mut() = Some(State::new(&app)));
    wire(&app);

    // open a folder or file passed on the command line: `rcode <path>`
    if let Some(arg) = std::env::args_os().nth(1) {
        let p = std::path::PathBuf::from(arg);
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        with_state(|s| {
            if p.is_dir() {
                s.open_local_folder_now(p);
            } else {
                s.open_local(&p);
            }
        });
    }

    app.invoke_focus_root();
    app.run()
}

pub(crate) fn wire(app: &App) {
    let w = app.as_weak();

    // ---- window ----
    {
        let w = w.clone();
        app.on_win_minimize(move || {
            if let Some(a) = w.upgrade() {
                a.window().set_minimized(true);
            }
        });
    }
    {
        let w = w.clone();
        app.on_win_toggle_maximize(move || {
            if let Some(a) = w.upgrade() {
                let m = a.window().is_maximized();
                a.window().set_maximized(!m);
            }
        });
    }
    app.on_win_close(|| {
        with_state(|s| s.request_quit());
    });
    app.window().on_close_requested(|| {
        with_state(|s| s.request_quit());
        slint::CloseRequestResponse::KeepWindowShown
    });

    // ---- keys ----
    app.on_global_key(|t, c, s, a| with_state(|st| st.global_key(&t, c, s, a)).unwrap_or(false));
    app.on_editor_key(|t, c, s, a| with_state(|st| st.editor_key(&t, c, s, a)).unwrap_or(false));
    app.on_tree_key(|t, c, s, a| with_state(|st| st.tree_key(&t, c, s, a)).unwrap_or(false));

    // ---- editor ----
    app.on_editor_edited(|text| {
        if with_state(|s| s.editor_edited(&text)).is_none() {
            // re-entrant (an edit we triggered ourselves): sync right after
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                with_state(|s| s.editor_edited(&text));
            });
        }
    });
    app.on_editor_cursor_moved(|c, a| {
        with_state(|s| s.editor_cursor_moved(c.max(0) as usize, a.max(0) as usize));
    });
    app.on_gutter_clicked(|line, shift| {
        with_state(|s| s.gutter_clicked(line, shift));
    });
    app.on_command(|id| {
        with_state(|s| s.run_command(&id));
    });

    // ---- tabs ----
    app.on_tab_clicked(|i| {
        with_state(|s| s.tab_clicked(i as usize));
    });
    app.on_tab_close(|i| {
        with_state(|s| s.request_close_tab(i as usize));
    });

    // ---- side bar ----
    app.on_activity(|v| {
        with_state(|s| s.activity(&v));
    });
    app.on_tree_clicked(|i| {
        with_state(|s| s.tree_clicked(i as usize));
    });
    app.on_tree_context(|i, x, y| {
        with_state(|s| s.tree_context(i, x, y));
    });
    app.on_explorer_action(|a| {
        with_state(|s| s.explorer_action(&a));
    });
    app.on_host_clicked(|i| {
        with_state(|s| s.host_clicked(i as usize));
    });
    app.on_remote_action(|a| {
        with_state(|s| s.remote_action(&a));
    });

    // ---- title bar / menus ----
    app.on_menu_open(|i, x| {
        with_state(|s| s.menu_open(i as usize, x));
    });
    app.on_popup_activated(|i| {
        with_state(|s| s.popup_activated(i as usize));
    });
    app.on_popup_dismiss(|| {
        with_state(|s| s.close_popup());
    });
    app.on_command_center_clicked(|| {
        with_state(|s| s.run_command("quickOpen"));
    });

    // ---- quick input ----
    app.on_quick_edited(|t| {
        with_state(|s| s.quick_edited(&t));
    });
    app.on_quick_accept(|i| {
        with_state(|s| s.quick_accept(i));
    });
    app.on_quick_cancel(|| {
        with_state(|s| s.quick_cancel());
    });

    // ---- find ----
    app.on_find_edited(|t| {
        with_state(|s| s.find_edited(&t));
    });
    app.on_find_next(|back| {
        with_state(|s| s.find_step(back));
    });
    app.on_replace_one(|| {
        with_state(|s| s.replace_one());
    });
    app.on_replace_all(|| {
        with_state(|s| s.replace_all());
    });
    app.on_find_close(|| {
        with_state(|s| s.find_close());
    });
    app.on_find_toggle_replace(|| {
        with_state(|s| s.find_toggle_replace());
    });
    app.on_find_toggle(|w| {
        with_state(|s| s.find_toggle(&w));
    });

    // ---- status bar / dialog / notifications ----
    app.on_status_clicked(|w| {
        with_state(|s| s.status_clicked(&w));
    });
    app.on_dialog_result(|i| {
        with_state(|s| s.dialog_result(i));
    });
    app.on_toast_close(|i| {
        with_state(|s| s.toast_close(i as usize));
    });
}
