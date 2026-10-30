//! The Files panel: everything attached to a task, or to one article
//! revision — the web app's `FilesPanel`, the same three-step upload.
//!
//! An upload is a ticket from the API, the bytes PUT straight to storage on
//! the presigned URL, then confirm, which is where the server checks what
//! really landed. Several files go one after another, as on the web: one
//! line of progress, and one fast upload rather than four slow ones.
//!
//! Files come from "Add a file" (the native picker) or a drop: the host view
//! takes `ExternalPaths` dropped anywhere on it (`drop_target`) and hands
//! them to `add_paths`, because a drop target you have to aim at is one
//! people miss.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::api::ApiError;
use crate::api::models::FileItem;
use crate::assets::Lucide;
use crate::session::OrgCtx;
use crate::ui::format::ago_long;

/// The server's default `attachment_max_bytes`. Checked here only so a huge
/// file isn't read into memory and sent just to be refused at confirm.
pub(crate) const MAX_BYTES: u64 = 50 * 1024 * 1024;

pub enum FilesEvent {
    AuthFailed(ApiError),
}

impl EventEmitter<FilesEvent> for FilesPanel {}

pub struct FilesPanel {
    ctx: OrgCtx,
    /// `{base}/files`; `None` until there is something to attach to.
    base: Option<String>,
    can_edit: bool,
    /// "task" or "article", for the drop hint.
    noun: &'static str,
    files: Option<Vec<FileItem>>,
    thumbnails: HashMap<String, Arc<Image>>,
    fetching: HashSet<String>,
    queue: Vec<PathBuf>,
    uploading: Option<String>,
    error: Option<String>,
    _tasks: Vec<Task<()>>,
}

impl FilesPanel {
    pub fn new(ctx: OrgCtx, noun: &'static str) -> Self {
        Self {
            ctx,
            base: None,
            can_edit: false,
            noun,
            files: None,
            thumbnails: HashMap::new(),
            fetching: HashSet::new(),
            queue: Vec::new(),
            uploading: None,
            error: None,
            _tasks: Vec::new(),
        }
    }

    /// Point the panel at a task or a revision; a new base reloads.
    pub fn set_target(&mut self, base: Option<String>, can_edit: bool, cx: &mut Context<Self>) {
        self.can_edit = can_edit;
        if base != self.base {
            self.base = base;
            self.files = None;
            self.error = None;
            self.reload(cx);
        }
        cx.notify();
    }

    pub fn can_upload(&self) -> bool {
        self.can_edit && self.base.is_some()
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        let client = self.ctx.client();
        self.spawn(cx, async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.files(&base) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(files) => {
                        this.files = Some(files);
                        this.fetch_thumbnails(cx);
                    }
                    Err(e) if e.is_auth() => cx.emit(FilesEvent::AuthFailed(e)),
                    Err(e) => {
                        this.files.get_or_insert_default();
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn spawn(
        &mut self,
        cx: &mut Context<Self>,
        f: impl AsyncFnOnce(WeakEntity<Self>, &mut AsyncApp) + 'static,
    ) {
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn(f));
    }

    /// Pictures are drawn from their thumbnail, fetched once per file; the
    /// worker may not have made one yet, so the original stands in.
    fn fetch_thumbnails(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, String)> = self
            .files
            .iter()
            .flatten()
            .filter(|f| f.is_image())
            .filter(|f| !self.thumbnails.contains_key(&f.id) && !self.fetching.contains(&f.id))
            .map(|f| {
                (
                    f.id.clone(),
                    f.thumbnail_url.clone().unwrap_or(f.url.clone()),
                )
            })
            .collect();
        for (id, url) in wanted {
            self.fetching.insert(id.clone());
            let client = self.ctx.client();
            self.spawn(cx, async move |this, cx| {
                let image = cx
                    .background_spawn(async move {
                        let bytes = client
                            .get_from_storage(&url)
                            .inspect_err(|e| log::debug!("thumbnail: {e}"))
                            .ok()?;
                        let format = sniff_image(&bytes)?;
                        Some(Arc::new(Image::from_bytes(format, bytes)))
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(image) = image {
                        this.thumbnails.insert(id, image);
                        cx.notify();
                    }
                })
                .ok();
            });
        }
    }

    pub fn pick_files(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        self.spawn(cx, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                this.update(cx, |this, cx| this.add_paths(paths, cx)).ok();
            }
        });
    }

    /// Queue files for upload; folders are skipped.
    pub fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if !self.can_upload() {
            return;
        }
        self.error = None;
        self.queue.extend(paths.into_iter().filter(|p| p.is_file()));
        self.upload_next(cx);
        cx.notify();
    }

    fn upload_next(&mut self, cx: &mut Context<Self>) {
        if self.uploading.is_some() || self.queue.is_empty() {
            return;
        }
        let Some(base) = self.base.clone() else {
            self.queue.clear();
            return;
        };
        let path = self.queue.remove(0);
        let name = file_name(&path);
        self.uploading = Some(name.clone());
        let client = self.ctx.client();
        let org_id = self.ctx.org_id().to_owned();
        self.spawn(cx, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let content_type = content_type_for(&path).ok_or_else(|| {
                        "this kind of file can't be attached — images, PDF, text, CSV, \
                         video and audio can"
                            .to_owned()
                    })?;
                    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                    if size > MAX_BYTES {
                        return Err(format!("it's over the {} MB limit", MAX_BYTES >> 20));
                    }
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    let ticket = client
                        .stage_file(&base, &file_name(&path), content_type)
                        .map_err(|e| e.to_string())?;
                    // The signed type, not our guess: the server may have
                    // normalised it, and the signature covers it exactly.
                    client
                        .put_to_storage(&ticket.upload_url, &ticket.content_type, &bytes)
                        .map_err(|e| e.to_string())?;
                    client
                        .confirm_attachment(&org_id, &ticket.attachment.id)
                        .map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.uploading = None;
                if let Err(e) = result {
                    this.error = Some(format!("{name} didn't upload: {e}"));
                }
                if this.queue.is_empty() {
                    this.reload(cx);
                } else {
                    this.upload_next(cx);
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn remove(&mut self, file_id: String, cx: &mut Context<Self>) {
        let Some(base) = self.base.clone() else {
            return;
        };
        let client = self.ctx.client();
        self.spawn(cx, async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.delete_file(&base, &file_id) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) | Err(ApiError::NotFound(_)) => this.reload(cx),
                    Err(e) => this.error = Some(format!("Couldn't remove that: {e}")),
                }
                cx.notify();
            })
            .ok();
        });
    }

    fn render_tile(&self, file: &FileItem, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let url = file.url.clone();
        let id = file.id.clone();
        let preview = match self.thumbnails.get(&file.id) {
            Some(image) => div()
                .w_full()
                .h(px(96.))
                .flex_none()
                .overflow_hidden()
                .bg(theme.muted)
                .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
                .into_any_element(),
            None => div()
                .w_full()
                .h(px(96.))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.muted.opacity(0.5))
                .child(
                    Icon::new(if file.content_type.starts_with("audio/") {
                        Lucide::AudioLines
                    } else {
                        Lucide::File
                    })
                    .size(px(28.))
                    .text_color(theme.muted_foreground),
                )
                .into_any_element(),
        };
        let who = match (&file.uploaded_by, file.created_at) {
            (Some(p), Some(at)) => format!("{} · {}", p.name(), ago_long(at)),
            (Some(p), None) => p.name(),
            (None, Some(at)) => ago_long(at),
            (None, None) => String::new(),
        };
        let removable = self.can_edit && !file.from_comment;
        div()
            .id(SharedString::from(format!("file-{}", file.id)))
            .group("file-tile")
            .relative()
            .w(px(168.))
            .overflow_hidden()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|s| s.bg(theme.accent.opacity(0.5)))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(who.clone()).build(window, cx)
            })
            .on_click(move |_, _, cx| cx.open_url(&url))
            .child(preview)
            .child(
                v_flex()
                    .p_2()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(file.filename.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .text_size(px(11.))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(file.size_bytes.map(format_bytes).unwrap_or_default())
                            .when(file.from_comment, |this| {
                                this.child(Icon::new(Lucide::MessageSquare).size(px(12.)))
                            }),
                    ),
            )
            .when(removable, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_1()
                        .right_1()
                        .invisible()
                        .group_hover("file-tile", |s| s.visible())
                        .child(
                            Button::new(SharedString::from(format!("rm-file-{}", file.id)))
                                .icon(Lucide::Trash)
                                .xsmall()
                                .tooltip(format!("Remove {}", file.filename))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove(id.clone(), cx)
                                })),
                        ),
                )
            })
    }
}

impl Render for FilesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let files = self.files.clone().unwrap_or_default();
        let noun = self.noun;
        let can_upload = self.can_upload();
        v_flex()
            .id("files-panel")
            .relative()
            .gap_3()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Lucide::Paperclip).small().text_color(theme.blue))
                    .child("FILES")
                    .when(!files.is_empty(), |this| {
                        this.child(
                            div()
                                .font_weight(FontWeight::NORMAL)
                                .font_family(theme.mono_font_family.clone())
                                .child(files.len().to_string()),
                        )
                    }),
            )
            .when_some(self.error.clone(), |this, e| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(theme.radius)
                        .bg(theme.danger.opacity(0.12))
                        .text_sm()
                        .child(e),
                )
            })
            .when(self.files.is_some() && files.is_empty(), |this| {
                this.child(div().text_sm().text_color(theme.muted_foreground).child(
                    if can_upload {
                        format!(
                            "Nothing attached yet. Drop files here to attach them to this {noun}."
                        )
                    } else {
                        "Nothing attached yet.".to_owned()
                    },
                ))
            })
            .when(!files.is_empty(), |this| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .children(files.iter().map(|f| self.render_tile(f, cx))),
                )
            })
            .when_some(self.uploading.clone(), |this, name| {
                let waiting = self.queue.len();
                this.child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Spinner::new().small())
                        .child(if waiting > 0 {
                            format!("Uploading {name}… ({waiting} more after this)")
                        } else {
                            format!("Uploading {name}…")
                        }),
                )
            })
            .when(can_upload, |this| {
                this.child(
                    h_flex().child(
                        Button::new("add-file")
                            .icon(Lucide::Paperclip)
                            .label("Add a file")
                            .small()
                            .outline()
                            .disabled(self.uploading.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
                    ),
                )
            })
    }
}

/// Makes `host` — a whole task or article view — take dropped files for
/// `panel`, tinted while a drag is over it.
pub fn drop_target<E: InteractiveElement + StatefulInteractiveElement>(
    host: E,
    panel: &Entity<FilesPanel>,
    cx: &App,
) -> E {
    if !panel.read(cx).can_upload() {
        return host;
    }
    let panel = panel.clone();
    host.drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().primary.opacity(0.04)))
        .on_drop(move |paths: &ExternalPaths, _, cx| {
            let paths = paths.paths().to_vec();
            panel.update(cx, |panel, cx| panel.add_paths(paths, cx));
        })
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into())
}

/// The types the server accepts (`ALLOWED_TYPES` in `models/conversation.py`),
/// by extension; anything else would only be refused.
pub(crate) fn content_type_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "text" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        _ => return None,
    })
}

pub(crate) fn sniff_image(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, 0xFF, ..] => Some(ImageFormat::Jpeg),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(ImageFormat::Webp),
        _ => None,
    }
}

pub fn format_bytes(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.0} KB", b as f64 / 1024.),
        b => format!("{:.1} MB", b as f64 / (1024. * 1024.)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn types_follow_the_servers_list() {
        assert_eq!(
            content_type_for(Path::new("a/Scan.PDF")),
            Some("application/pdf")
        );
        assert_eq!(content_type_for(Path::new("shot.jpeg")), Some("image/jpeg"));
        assert_eq!(content_type_for(Path::new("notes.md")), Some("text/plain"));
        assert_eq!(content_type_for(Path::new("archive.zip")), None);
        assert_eq!(content_type_for(Path::new("Makefile")), None);
    }

    #[::core::prelude::v1::test]
    fn sizes_read_like_the_web() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 + 300_000), "3.3 MB");
    }

    #[::core::prelude::v1::test]
    fn images_are_told_by_their_bytes() {
        assert_eq!(sniff_image(b"\x89PNG\r\n"), Some(ImageFormat::Png));
        assert_eq!(sniff_image(b"\xFF\xD8\xFF\xE0"), Some(ImageFormat::Jpeg));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8"), Some(ImageFormat::Webp));
        assert_eq!(sniff_image(b"%PDF"), None);
    }
}
