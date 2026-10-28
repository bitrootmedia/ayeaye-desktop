//! The module rail. The organisation's modules on top; the person's own —
//! sparks, reminders, the inbox, which ignore the organisation — below.

use crate::assets::Lucide;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
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

    pub fn all() -> impl Iterator<Item = Module> {
        Self::ORG.iter().chain(Self::PERSONAL).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
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
