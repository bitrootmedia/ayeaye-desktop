//! Icons: GPUI Kit's default set plus the few Lucide icons this app adds.
//! The logo is the web app's own anchor, from the same Lucide catalog — the
//! menu bar app draws the same one.

use std::borrow::Cow;

use gpui_kit::*;

pub use gpui_kit::assets::IconName as Lucide;

pub const LOGO: Lucide = Lucide::Anchor;

gpui_kit::assets::icon_assets!(
    Extra,
    [
        Anchor,
        NotebookPen,
        ListTodo,
        BookOpen,
        Sparkles,
        LogOut,
        UserPlus,
        CloudOff,
        Server,
        Building2,
        SquarePen,
        Check,
        Trash,
        SquareCheck,
        CalendarDays,
        Bell,
        Inbox,
        Lightbulb,
        Search,
        Timer,
        CalendarCheck,
        BookText,
        Library,
        MessageSquare,
        TextAlignStart,
        ListChecks,
        Table,
        Eye,
        Paperclip,
        AudioLines,
        File,
        FolderKanban,
        Archive,
        ArchiveRestore,
        Users,
        UserCog,
        Clock,
        Plane,
        MessageSquareQuote,
        KeyRound,
        ShieldCheck,
        Send,
        Webhook,
        Mail,
        Plug,
        House,
        Megaphone,
        Pin,
        TriangleAlert,
        CalendarClock,
        Activity,
        MessageCircle,
        MessagesSquare,
        BellOff,
        Reply,
        PinOff,
        Pencil,
        Bookmark,
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match Extra.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        all.extend(Extra.list(path)?);
        Ok(all)
    }
}
