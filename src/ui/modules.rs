//! The module rail. The organisation's modules on top; the person's own —
//! sparks, reminders, the inbox, which ignore the organisation — below.
//! Chats sits apart, under Home and Calendar: it has its own key (⌘⇧M)
//! rather than taking a number from the modules after it.

use crate::assets::Lucide;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    Chats,
    Notes,
    Tasks,
    Projects,
    People,
    Planner,
    Knowledge,
    Sparks,
    Reminders,
    Inbox,
}

impl Module {
    pub const ORG: &'static [Module] = &[
        Module::Notes,
        Module::Tasks,
        Module::Projects,
        Module::People,
        Module::Planner,
        Module::Knowledge,
    ];
    pub const PERSONAL: &'static [Module] = &[Module::Sparks, Module::Reminders, Module::Inbox];

    /// In ⌘⌥1–9 order; Chats comes last, past the ninth, so the numbers
    /// people already know stay where they were.
    pub fn all() -> impl Iterator<Item = Module> {
        Self::ORG
            .iter()
            .chain(Self::PERSONAL)
            .chain([&Module::Chats])
            .copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Module::Chats => "Chats",
            Module::Notes => "Notes",
            Module::Tasks => "Tasks",
            Module::Projects => "Projects",
            Module::People => "People",
            Module::Planner => "My planner",
            Module::Knowledge => "Knowledge base",
            Module::Sparks => "Sparks",
            Module::Reminders => "Reminders",
            Module::Inbox => "Inbox",
        }
    }

    pub fn icon(self) -> Lucide {
        match self {
            Module::Chats => Lucide::MessageCircle,
            Module::Notes => Lucide::NotebookPen,
            Module::Tasks => Lucide::ListTodo,
            Module::Projects => Lucide::FolderKanban,
            Module::People => Lucide::Users,
            Module::Planner => Lucide::CalendarDays,
            Module::Knowledge => Lucide::Library,
            Module::Sparks => Lucide::Lightbulb,
            Module::Reminders => Lucide::Bell,
            Module::Inbox => Lucide::Inbox,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Module::Chats => "chats",
            Module::Notes => "notes",
            Module::Tasks => "tasks",
            Module::Projects => "projects",
            Module::People => "people",
            Module::Planner => "planner",
            Module::Knowledge => "kb",
            Module::Sparks => "sparks",
            Module::Reminders => "reminders",
            Module::Inbox => "inbox",
        }
    }

    pub fn from_key(key: &str) -> Option<Module> {
        Self::all().find(|m| m.key() == key)
    }
}
