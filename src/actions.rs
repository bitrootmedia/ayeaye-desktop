//! Every command the app answers to, its key bindings, and the macOS menu
//! bar. `secondary` is ⌘ on macOS and Ctrl elsewhere.

use gpui_kit::component::input;
use gpui_kit::*;

gpui_kit::actions!(
    ayeaye,
    [
        NewNote,
        NewTask,
        CaptureSpark,
        Search,
        CloseTab,
        NextTab,
        PrevTab,
        ReopenClosedTab,
        ToggleSidebar,
        FindNote,
        FocusNotes,
        SaveNow,
        DeleteNote,
        Refresh,
        Quit,
        About,
        SelectNext,
        SelectPrev,
        Cancel,
    ]
);

#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = ayeaye, no_json)]
pub struct ActivateTab(pub usize);

/// ⌘⌥1…7: jump to a module on the rail.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = ayeaye, no_json)]
pub struct ShowModule(pub usize);

pub fn bind_keys(cx: &mut App) {
    let ws = Some("Workspace");
    let mut bindings = vec![
        KeyBinding::new("secondary-n", NewNote, ws),
        KeyBinding::new("secondary-shift-n", NewTask, ws),
        KeyBinding::new("secondary-j", CaptureSpark, ws),
        KeyBinding::new("secondary-shift-f", Search, ws),
        KeyBinding::new("secondary-w", CloseTab, ws),
        KeyBinding::new("ctrl-tab", NextTab, ws),
        KeyBinding::new("ctrl-shift-tab", PrevTab, ws),
        KeyBinding::new("secondary-shift-]", NextTab, ws),
        KeyBinding::new("secondary-shift-[", PrevTab, ws),
        KeyBinding::new("secondary-alt-right", NextTab, ws),
        KeyBinding::new("secondary-alt-left", PrevTab, ws),
        KeyBinding::new("secondary-shift-t", ReopenClosedTab, ws),
        KeyBinding::new("secondary-b", ToggleSidebar, ws),
        KeyBinding::new("secondary-p", FindNote, ws),
        KeyBinding::new("secondary-k", FindNote, ws),
        KeyBinding::new("secondary-shift-e", FocusNotes, ws),
        KeyBinding::new("secondary-s", SaveNow, ws),
        KeyBinding::new("secondary-shift-backspace", DeleteNote, ws),
        KeyBinding::new("secondary-r", Refresh, ws),
        KeyBinding::new("secondary-q", Quit, None),
        // Lists with a filter field: the arrows move the selection, not the
        // caret. "X > Input" outranks the Input's own up/down.
        KeyBinding::new("up", SelectPrev, Some("NotesPanel > Input")),
        KeyBinding::new("down", SelectNext, Some("NotesPanel > Input")),
        KeyBinding::new("escape", Cancel, Some("NotesPanel")),
        KeyBinding::new("up", SelectPrev, Some("QuickOpen > Input")),
        KeyBinding::new("down", SelectNext, Some("QuickOpen > Input")),
        KeyBinding::new("ctrl-p", SelectPrev, Some("QuickOpen > Input")),
        KeyBinding::new("ctrl-n", SelectNext, Some("QuickOpen > Input")),
        KeyBinding::new("escape", Cancel, Some("QuickOpen")),
    ];
    for n in 1..=9 {
        bindings.push(KeyBinding::new(
            &format!("secondary-{n}"),
            ActivateTab(n - 1),
            ws,
        ));
    }
    cx.bind_keys(bindings);
}

pub fn set_menus(cx: &mut App) {
    cx.set_menus(vec![
        Menu::new("Ayeaye").items([
            MenuItem::action("About Ayeaye", About),
            MenuItem::separator(),
            MenuItem::action("Quit Ayeaye", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Note", NewNote),
            MenuItem::action("New Task…", NewTask),
            MenuItem::action("Capture a Spark…", CaptureSpark),
            MenuItem::action("Find Note…", FindNote),
            MenuItem::action("Search…", Search),
            MenuItem::separator(),
            MenuItem::action("Save Now", SaveNow),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
            MenuItem::separator(),
            MenuItem::action("Delete Note…", DeleteNote),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", input::Undo),
            MenuItem::action("Redo", input::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Notes", ShowModule(0)),
            MenuItem::action("Tasks", ShowModule(1)),
            MenuItem::action("My Planner", ShowModule(2)),
            MenuItem::action("Knowledge Base", ShowModule(3)),
            MenuItem::action("Sparks", ShowModule(4)),
            MenuItem::action("Reminders", ShowModule(5)),
            MenuItem::action("Inbox", ShowModule(6)),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Focus Notes List", FocusNotes),
            MenuItem::action("Refresh", Refresh),
            MenuItem::separator(),
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Previous Tab", PrevTab),
        ]),
        Menu::new("Window").items([]),
    ]);
}
