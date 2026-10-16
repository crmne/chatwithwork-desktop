//! The website's layered instruction editor, built with native text controls.

use cww::workspace::{InstructionChange, Layer};
use egui::{Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::controls::{Button, Kind, Size};
use super::icons::{Icon, Images};
use super::paint;
use super::state::Command;
use super::tokens::{Palette, Type, scale};
use super::widgets;
use super::workspace::{Editor, Instructions};

pub struct Outcome {
    pub close: bool,
    pub open_chat: Option<u64>,
    pub commands: Vec<Command>,
    pub event: Option<super::sidebar::Event>,
    pub download: Option<cww::templates::File>,
}

pub fn show(
    ui: &mut Ui,
    rect: Rect,
    page: &mut Instructions,
    tabs: &[cww::tui::chat::SettingsLink],
    p: &Palette,
    images: &Images,
) -> Outcome {
    let mut outcome = Outcome {
        close: false,
        open_chat: None,
        commands: vec![],
        event: None,
        download: None,
    };
    if page.project.is_some() {
        let width = (rect.width() - 48.0).clamp(1.0, 768.0);
        let bounds = Rect::from_min_max(
            pos2(rect.center().x - width / 2.0, rect.top() + 48.0),
            pos2(rect.center().x + width / 2.0, rect.bottom()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bounds));
        child.set_clip_rect(bounds);
        egui::ScrollArea::vertical()
            .id_salt(("project-instructions", page.generation))
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.set_width(ui.available_width());
                content(ui, page, p, images, &mut outcome);
                ui.add_space(48.0);
            });
    } else {
        let header = page
            .context
            .as_ref()
            .map(|c| format!("{} · {}", c.email, c.account.name));
        let event = super::settings::Shell {
            rect,
            generation: page.generation,
            header: header.as_deref(),
            active: "instructions",
            tabs,
            busy: page.working(),
            palette: p,
            images,
        }
        .show(ui, |ui| content(ui, page, p, images, &mut outcome));
        if event.is_some() {
            outcome.event = event;
        }
    }
    if page.confirm_close {
        let closed = super::controls::modal(
            ui.ctx(),
            Id::new("discard-instructions"),
            420.0,
            180.0,
            p,
            |ui, rect| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                text(
                    &mut child,
                    "Discard unsaved instructions?",
                    Type::sans(18.0, 27.0).weight(600.0),
                    p.ink,
                );
                child.add_space(12.0);
                text(
                    &mut child,
                    "Your saved instructions will stay as they are.",
                    scale::SMALL,
                    p.ink_muted,
                );
                child.add_space(20.0);
                child.horizontal(|ui| {
                    if button(
                        ui,
                        "instructions-keep",
                        "Keep editing",
                        Kind::Subtle,
                        true,
                        p,
                        images,
                    ) {
                        page.confirm_close = false;
                    }
                    if button(
                        ui,
                        "instructions-discard",
                        "Discard changes",
                        Kind::Error,
                        true,
                        p,
                        images,
                    ) {
                        outcome.close = true;
                    }
                });
            },
        );
        if !closed {
            page.confirm_close = false;
        }
    }
    outcome
}

fn content(ui: &mut Ui, page: &mut Instructions, p: &Palette, images: &Images, out: &mut Outcome) {
    ui.set_width(ui.available_width());
    let project = page.context.as_ref().and_then(|c| c.project.clone());
    if page.project.is_some() {
        let back = Button::new(
            Kind::Subtle,
            Size::Small,
            project.as_ref().map_or("Back to project", |p| &p.name),
        )
        .name("Back to project")
        .enabled(!page.working());
        let (rect, _) = ui.allocate_exact_size(vec2(back.width(ui), back.height()), Sense::hover());
        if back
            .show(ui, Id::new("instructions-back"), rect, p, images)
            .clicked()
        {
            if page.dirty() {
                page.confirm_close = true;
            } else {
                out.close = true;
            }
        }
        ui.add_space(16.0);
    }
    text(
        ui,
        "Instructions",
        Type::sans(if page.project.is_some() { 24.0 } else { 15.0 }, 24.0).weight(600.0),
        p.ink,
    );
    ui.add_space(8.0);
    let intro = project.as_ref().map_or_else(
 || "Notes the assistant reads at the start of every chat, so you don't repeat yourself: who you are, who's who, where things live, and how you like answers. Like a README for your work. The assistant can suggest additions too, and changes nothing until you approve.".into(),
 |project| format!("Notes the assistant reads at the start of every chat in {}: what it is, who does what, where things live, and what's been decided. Everyone on the project but guests reads them{}.", project.name, if project.archived {""} else {", and can edit them"}));
    text(ui, &intro, scale::SMALL, p.ink_muted);
    ui.add_space(24.0);
    if page.editors.is_empty() && page.busy {
        text(ui, "Loading instructions…", scale::SMALL, p.ink_muted);
    }
    if let Some(error) = &page.error {
        text(ui, error, scale::SMALL, p.negative_ink());
        ui.add_space(12.0);
    }
    if let Some(notice) = &page.notice {
        text(ui, notice, scale::SMALL, p.ink_muted);
        ui.add_space(12.0);
    }
    if page.project.is_none() && !page.editors.is_empty() {
        stack(ui, page, p, images, out);
    }
    let working = page.working();
    for editor in &mut page.editors {
        let layer = editor.instruction.layer;
        if page.project.is_some() && layer != Layer::Project {
            continue;
        }
        let section_top = ui.cursor().min;
        if page.project.is_none() {
            ui.add_space(48.0);
            text(
                ui,
                &heading(editor),
                Type::sans(15.0, 22.5).weight(600.0),
                p.ink,
            );
            ui.add_space(4.0);
            let description = if layer == Layer::Personal {
                "Read last, in every chat you start or reply to here, so they win on how to answer you. Only you and the assistant, with your approval, change them; teammates' chats never read them.".into()
            } else {
                format!(
                    "Read first, in every chat in {}, for everyone but guests. Keep it to what's true for the whole organization; projects and people add their own.",
                    editor.instruction.owner
                )
            };
            text(ui, &description, scale::SMALL, p.ink_muted);
            ui.add_space(16.0);
        }
        if let Some(change) = editor_ui(ui, editor, working, p, images) {
            page.busy = true;
            page.error = None;
            page.notice = None;
            out.commands.push(Command::SaveInstruction(
                page.generation,
                layer,
                change,
                page.project,
            ));
        }
        let now = ui.input(|i| i.time);
        if editor.counting.is_none() && editor.changed_at.is_some_and(|at| now - at >= 0.5) {
            editor.changed_at = None;
            editor.counting = Some(editor.body.clone());
            out.commands.push(Command::CountInstruction(
                page.generation,
                layer,
                editor.body.clone(),
            ));
        }
        if editor.changed_at.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if page.scroll_to == Some(layer) {
            ui.scroll_to_rect_animation(
                Rect::from_min_size(section_top, vec2(ui.available_width(), 1.0)),
                Some(egui::Align::Min),
                egui::style::ScrollAnimation::none(),
            );
            page.scroll_to = None;
        }
    }
    if page.project.is_some() {
        if let Some(templates) = &mut page.templates {
            ui.add_space(48.0);
            let result = ui
                .add_enabled_ui(!page.busy, |ui| {
                    super::templates::inline(ui, templates, p, images)
                })
                .inner;
            out.commands.extend(result.commands);
            out.download = result.download;
        }
        if !page.editors.is_empty() {
            ui.add_space(48.0);
            text(
                ui,
                "What this project's chats read",
                Type::sans(15.0, 22.5).weight(600.0),
                p.ink,
            );
            ui.add_space(16.0);
            stack(ui, page, p, images, out);
        }
    } else {
        memory(ui, page, p, images, out);
    }
}

fn stack(ui: &mut Ui, page: &mut Instructions, p: &Palette, images: &Images, out: &mut Outcome) {
    let mut rows: Vec<Option<usize>> = (0..page.editors.len()).map(Some).collect();
    if page.project.is_none()
        && page
            .editors
            .iter()
            .any(|e| e.instruction.layer == Layer::Organization)
    {
        let at = page
            .editors
            .iter()
            .position(|e| e.instruction.layer == Layer::Personal)
            .unwrap_or(rows.len());
        rows.insert(at, None);
    }
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (index, row) in rows.into_iter().enumerate() {
                if index > 0 {
                    rule(ui, p);
                }
                stack_row(ui, page, row, p, images, out);
            }
        });
    ui.add_space(12.0);
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
        images.icon_at(ui.painter(),r.center(),14.0,Icon::ArrowDown,p.ink_faint);
        ui.add_space(8.0);
        text(ui, "Read top to bottom. Where two disagree, the more specific wins: a project's over the organization's, and yours over both on how to answer you.", scale::SMALLER, p.ink_muted);
    });
}

fn stack_row(
    ui: &mut Ui,
    page: &mut Instructions,
    row: Option<usize>,
    p: &Palette,
    images: &Images,
    out: &mut Outcome,
) {
    let editor = row.map(|i| &page.editors[i]);
    let here =
        page.project.is_some() && editor.is_some_and(|e| e.instruction.layer == Layer::Project);
    let name = editor.map_or_else(|| "Each project's instructions".into(), heading);
    let label = if here {
        "This page"
    } else {
        editor.map_or("Projects", |e| {
            if e.instruction.editable {
                "Edit"
            } else {
                "Read"
            }
        })
    };
    let action = Button::new(Kind::Subtle, Size::Small, label)
        .paper(egui::Color32::TRANSPARENT, egui::Color32::TRANSPARENT)
        .enabled(!page.working());
    egui::Frame::new().inner_margin(16).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let (node, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            ui.painter().rect_filled(node, 8.0, p.canvas);
            ui.painter().rect_stroke(node, 8.0, Stroke::new(1.0, p.line), egui::StrokeKind::Inside);
            let icon = editor.map_or(Icon::Folders, |e| match e.instruction.layer {
                Layer::Organization => Icon::Buildings,
                Layer::Project => Icon::FolderSimple,
                Layer::Personal => Icon::User,
            });
            images.icon_at(ui.painter(), node.center(), 16.0, icon, p.ink_muted);
            ui.add_space(12.0);
            let width = (ui.available_width() - action.width(ui) - 28.0).max(1.0);
            ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_width(width);
                text(ui, &name, scale::SMALL.weight(550.0), p.ink);
                let note = editor.map_or("That project's chats. Open a project and choose Instructions to write them.", |e| match e.instruction.layer {
                    Layer::Organization => "Every chat in the organization, for everyone but guests",
                    Layer::Project => "This project's chats, for everyone on it but guests",
                    Layer::Personal => "Every chat you start or reply to",
                });
                text(ui, note, scale::SMALLER, p.ink_muted);
                if let Some(editor) = editor {
                    let count = if editor.instruction.body.is_empty() { "empty".into() } else { format!("{} tokens", number(editor.instruction.tokens)) };
                    text(ui, &count, scale::MICRO, p.ink_faint);
                }
            });
            ui.add_space(12.0);
            if here {
                text(ui, label, scale::MICRO, p.ink_faint);
            } else {
                let (r, _) = ui.allocate_exact_size(vec2(action.width(ui) + 16.0, action.height()), Sense::hover());
                let clicked = action.name(&format!("Open {name}")).show(ui, Id::new(("instruction-jump",row)),r,p,images).clicked();
                images.icon_at(ui.painter(),pos2(r.right()-8.0,r.center().y),14.0,Icon::ArrowRight,p.ink);
                if clicked {
                    if let Some(editor) = editor {
                        if page.project.is_some() {
                            out.event = Some(super::sidebar::Event::Instructions(editor.instruction.layer));
                        } else { page.scroll_to = Some(editor.instruction.layer); }
                    } else { out.event = Some(super::sidebar::Event::Projects(cww::projects::Request::List)); }
                }
            }
        });
    });
}

fn memory(ui: &mut Ui, page: &mut Instructions, p: &Palette, images: &Images, out: &mut Outcome) {
    let Some(memory) = &page.memory else {
        return;
    };
    ui.add_space(48.0);
    text(ui, "Memory", Type::sans(15.0, 22.5).weight(600.0), p.ink);
    ui.add_space(4.0);
    text(
        ui,
        "Small facts the assistant noticed about you and keeps for your later chats here, like your role or how you like answers. Each save shows in its chat with Undo. Only you see them, and only your chats read them.",
        scale::SMALL,
        p.ink_muted,
    );
    ui.add_space(16.0);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(12.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut enabled = memory.enabled;
            if memory_choice(ui, &mut enabled, !page.busy, p) {
                page.busy = true;
                out.commands
                    .push(Command::MemoryPreference(page.generation, enabled));
            }
            for entry in &memory.entries {
                rule(ui, p);
                let (open, forget) = memory_entry(ui, entry, !page.busy, p, images);
                if open {
                    out.open_chat = entry.chat;
                }
                if forget {
                    page.busy = true;
                    out.commands
                        .push(Command::MemoryForget(page.generation, entry.id));
                }
            }
        });
    if memory.entries.is_empty() {
        ui.add_space(16.0);
        egui::Frame::new()
            .stroke(Stroke::new(1.0, p.line))
            .corner_radius(12.0)
            .inner_margin(24)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (icon, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                images.icon_at(ui.painter(), icon.center(), 24.0, Icon::Brain, p.ink_muted);
                ui.add_space(8.0);
                text(ui, "Nothing remembered yet", scale::SMALL.weight(550.0), p.ink);
                text(ui, "Tell the assistant about your work as you chat, and it keeps what will help later.", scale::SMALLER, p.ink_muted);
            });
    }
}

fn memory_choice(ui: &mut Ui, value: &mut bool, enabled: bool, p: &Palette) -> bool {
    let mut changed = false;
    egui::Frame::new().inner_margin(16).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let width = (ui.available_width() - 52.0).max(1.0);
            ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_width(width);
                text(ui, "Let the assistant remember things", scale::SMALL.weight(550.0), p.ink);
                text(ui, "Off, it saves nothing new and your chats leave out what it kept, until you turn it back on or forget them.", scale::SMALLER, p.ink_muted);
            });
            ui.add_space(16.0);
            changed = super::controls::switch(ui, value, "Let the assistant remember things", enabled, p).changed();
        });
    });
    changed
}

fn memory_entry(
    ui: &mut Ui,
    entry: &cww::workspace::Remembered,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> (bool, bool) {
    let mut open = false;
    let mut forget = false;
    egui::Frame::new().inner_margin(16).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let width = (ui.available_width() - 48.0).max(1.0);
            ui.allocate_ui_with_layout(
                vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(width);
                    text(ui, &entry.body, scale::SMALL, p.ink);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        text(ui, &entry.date, scale::MICRO, p.ink_faint);
                        if entry.chat.is_some() {
                            open = ui
                                .add_enabled(
                                    enabled,
                                    egui::Link::new(
                                        egui::RichText::new("From a chat")
                                            .size(11.0)
                                            .color(p.ink_faint),
                                    ),
                                )
                                .clicked();
                        }
                    });
                },
            );
            ui.add_space(16.0);
            let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            forget = widgets::IconButton {
                icon: Icon::Trash,
                icon_size: 16.0,
                label: "Forget",
                tooltip: Some("Forget this memory"),
                color: p.ink_muted,
                hover_color: p.ink,
                wash: 7.0,
                radius: 8.0,
                enabled,
            }
            .show(ui, Id::new(("forget-memory", entry.id)), rect, images, p)
            .clicked();
        });
    });
    (open, forget)
}

fn heading(editor: &Editor) -> String {
    if editor.instruction.layer == Layer::Personal {
        "Your instructions".into()
    } else {
        format!("{}'s instructions", editor.instruction.owner)
    }
}

fn editor_ui(
    ui: &mut Ui,
    editor: &mut Editor,
    busy: bool,
    p: &Palette,
    images: &Images,
) -> Option<InstructionChange> {
    let mut change = None;
    let id = Id::new(("instructions", editor.instruction.layer.path()));
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(16.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                        images.icon_at(
                            ui.painter(),
                            rect.center(),
                            16.0,
                            Icon::Notebook,
                            p.ink_faint,
                        );
                        text(ui, "Instructions", Type::mono(13.0, 18.0), p.ink);
                        if !editor.instruction.editable {
                            text(ui, "Read only", scale::CAPTION, p.ink_muted);
                        }
                        if let Some(revision) = editor.instruction.revisions.first() {
                            text(
                                ui,
                                &format!(
                                    "Edited {} by {}",
                                    revision.age,
                                    if revision.author == "You" {
                                        "you"
                                    } else {
                                        &revision.author
                                    }
                                ),
                                Type::mono(11.0, 16.0),
                                p.ink_faint,
                            );
                        }
                    });
                });
            rule(ui, p);
            egui::Frame::new().inner_margin(16).show(ui, |ui| {
                if editor.instruction.editable {
                    let response = ui.add_enabled(
                        !busy,
                        egui::TextEdit::multiline(&mut editor.body)
                            .id(id.with("body"))
                            .font(Type::mono(13.0, 22.1).font_id())
                            .text_color(p.ink)
                            .frame(egui::Frame::NONE)
                            .desired_rows(12)
                            .desired_width(f32::INFINITY)
                            .hint_text(
                                egui::RichText::new(&editor.instruction.placeholder)
                                    .font(Type::mono(13.0, 22.1).font_id())
                                    .color(p.ink_faint),
                            ),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::TextEdit,
                            !busy,
                            format!("{}, in Markdown", heading(editor)),
                        )
                    });
                    if response.changed() {
                        editor.changed_at = Some(ui.input(|i| i.time));
                        editor.count_error = None;
                    }
                    if response.has_focus() {
                        widgets::focus_ring(ui, response.rect.expand(16.0), 10.0, p);
                    }
                    if response.has_focus()
                        && ui.input_mut(|i| {
                            i.consume_shortcut(&egui::KeyboardShortcut::new(
                                egui::Modifiers::COMMAND,
                                egui::Key::Enter,
                            ))
                        })
                        && !busy
                        && !over_limit(editor)
                    {
                        change = Some(InstructionChange::Edit {
                            body: editor.body.clone(),
                        });
                    }
                    if editor.body.trim().is_empty() {
                        ui.add_space(12.0);
                        text(
                            ui,
                            &format!(
                                "Not sure what to write? Start from a few headings: {}.",
                                editor.instruction.template_topics
                            ),
                            scale::SMALLER,
                            p.ink_muted,
                        );
                        ui.add_space(8.0);
                        if button(
                            ui,
                            id.with("template"),
                            "Use a template",
                            Kind::Subtle,
                            !busy,
                            p,
                            images,
                        ) {
                            editor.body.clone_from(&editor.instruction.template);
                            editor.changed_at = Some(ui.input(|i| i.time));
                        }
                    }
                } else {
                    text(
                        ui,
                        if editor.body.is_empty() {
                            "Nothing written yet."
                        } else {
                            &editor.body
                        },
                        Type::mono(13.0, 22.1),
                        p.ink,
                    );
                    if let Some(note) = &editor.instruction.editors_note {
                        ui.add_space(16.0);
                        text(ui, note, scale::SMALLER, p.ink_muted);
                    }
                }
            });
            if editor.instruction.editable {
                rule(ui, p);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let known = editor.counted_body == editor.body;
                            let fraction =
                                editor.tokens as f32 / editor.instruction.token_limit.max(1) as f32;
                            let tone = if fraction > 1.0 {
                                p.negative_ink()
                            } else if fraction >= 0.85 {
                                p.orange
                            } else {
                                p.ink_faint
                            };
                            let (track, _) =
                                ui.allocate_exact_size(vec2(56.0, 18.0), Sense::hover());
                            let track = Rect::from_center_size(track.center(), vec2(56.0, 4.0));
                            ui.painter().rect_filled(track, 2.0, p.line);
                            if known {
                                ui.painter().rect_filled(
                                    Rect::from_min_size(
                                        track.min,
                                        vec2(track.width() * fraction.min(1.0), 4.0),
                                    ),
                                    2.0,
                                    tone,
                                );
                            }
                            let count = if known {
                                format!(
                                    "{} of {} tokens",
                                    number(editor.tokens),
                                    number(editor.instruction.token_limit)
                                )
                            } else if editor.count_error.is_some() {
                                "Count unavailable".into()
                            } else {
                                "Counting…".into()
                            };
                            text(ui, &count, Type::mono(11.0, 16.0), tone);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if button(
                                        ui,
                                        id.with("save"),
                                        "Save",
                                        Kind::Primary,
                                        !busy && !over_limit(editor),
                                        p,
                                        images,
                                    ) {
                                        change = Some(InstructionChange::Edit {
                                            body: editor.body.clone(),
                                        });
                                    }
                                    text(
                                        ui,
                                        if cfg!(target_os = "macos") {
                                            "⌘ Enter"
                                        } else {
                                            "Ctrl Enter"
                                        },
                                        Type::mono(11.0, 16.0),
                                        p.ink_faint,
                                    );
                                    if editor.dirty() {
                                        text(
                                            ui,
                                            "Unsaved changes",
                                            Type::mono(11.0, 16.0),
                                            p.ink_muted,
                                        );
                                    }
                                },
                            );
                        });
                    });
            }
        });
    if editor.instruction.editable && editor.instruction.revisions.len() > 1 {
        ui.add_space(8.0);
        if button(
            ui,
            id.with("history"),
            &format!(
                "Earlier versions ({})",
                editor.instruction.revisions.len() - 1
            ),
            Kind::Subtle,
            true,
            p,
            images,
        ) {
            editor.history = !editor.history;
        }
        if editor.history {
            for (index, revision) in editor.instruction.revisions.iter().enumerate() {
                ui.add_space(12.0);
                text(
                    ui,
                    &format!(
                        "{}{}",
                        revision.author,
                        if index == 0 { " · Current" } else { "" }
                    ),
                    scale::SMALL.weight(560.0),
                    p.ink,
                );
                text(
                    ui,
                    &format!(
                        "{} · {} tokens{}",
                        revision.age,
                        number(revision.tokens),
                        revision
                            .changes
                            .as_ref()
                            .map_or(String::new(), |s| format!(" · {s}"))
                    ),
                    scale::MICRO,
                    p.ink_faint,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if button(
                        ui,
                        id.with(("peek", revision.id)),
                        if editor.expanded.contains(&revision.id) {
                            "Hide text"
                        } else {
                            "Show text"
                        },
                        Kind::Subtle,
                        true,
                        p,
                        images,
                    ) && !editor.expanded.remove(&revision.id)
                    {
                        editor.expanded.insert(revision.id);
                    }
                    if index > 0
                        && button(
                            ui,
                            id.with(("restore", revision.id)),
                            "Bring back",
                            Kind::Subtle,
                            !busy,
                            p,
                            images,
                        )
                    {
                        change = Some(InstructionChange::Restore {
                            revision_id: revision.id,
                        });
                    }
                });
                if editor.expanded.contains(&revision.id) {
                    ui.add_space(8.0);
                    text(
                        ui,
                        if revision.body.is_empty() {
                            "Empty"
                        } else {
                            &revision.body
                        },
                        Type::mono(12.0, 20.4),
                        p.ink_muted,
                    );
                }
                ui.add_space(8.0);
                rule(ui, p);
            }
        }
    }
    change
}

fn over_limit(editor: &Editor) -> bool {
    editor.counted_body == editor.body && editor.tokens > editor.instruction.token_limit
}

fn number(value: usize) -> String {
    let mut out = String::new();
    for (i, ch) in value.to_string().chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn rule(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.line));
}

fn text(ui: &mut Ui, text: &str, ty: Type, color: egui::Color32) {
    ui.add(egui::Label::new(paint::job(text, ty, color, ui.available_width())).wrap());
}

fn button(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    kind: Kind,
    enabled: bool,
    p: &Palette,
    images: &Images,
) -> bool {
    let button = Button::new(kind, Size::Small, label).enabled(enabled);
    let (rect, _) = ui.allocate_exact_size(vec2(button.width(ui), button.height()), Sense::hover());
    button.show(ui, Id::new(id), rect, p, images).clicked()
}
