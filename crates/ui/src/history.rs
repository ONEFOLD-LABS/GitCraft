//! The history view (docs/ux/layout.md): commits loaded page by page on a background thread,
//! laid out as a graph ([`gitcraft_graph`]) and drawn as a virtualised table with a refs column,
//! the graph, message, author and date. A work-in-progress row sits on top while the working
//! tree has changes.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::epaint::CubicBezierShape;
use egui::{Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use gitcraft_core::Reader;
use gitcraft_git::{Commit, GitError, Oid, Ref, RefKind, escape};
use gitcraft_graph::{Graph, Row as GraphRow};

use crate::i18n::{tr, trf};
use crate::theme::Tokens;

/// Commits read per request.
pub(crate) const PAGE: usize = 2000;
/// Height of one history row.
pub(crate) const ROW_H: f32 = 28.0;
const LANE_W: f32 = 24.0;
/// Radius of a commit's author disc on the graph.
const NODE_R: f32 = 9.5;
/// Height of the column header above the rows.
const HEADER_H: f32 = 22.0;
/// Lanes drawn at most; wider histories are cut at the right edge of the graph column.
const MAX_LANES: usize = 24;
const REFS_W: f32 = 170.0;
const DATE_W: f32 = 76.0;
const AUTHOR_W: f32 = 140.0;
/// Below this row width the author column is left out, so the message keeps its room.
const AUTHOR_MIN_ROW: f32 = 760.0;

/// Reads pages of the log on its own thread, one page per request. Dropping it ends the thread
/// (and with it `git log`).
struct Loader {
    want: Sender<usize>,
    pages: Receiver<(Result<Vec<Commit>, GitError>, bool)>,
}

impl Loader {
    fn start(repo: Reader, ctx: egui::Context) -> Result<Loader, String> {
        let (want, want_rx) = channel::<usize>();
        let (tx, pages) = channel();
        std::thread::Builder::new()
            .name("gitcraft-log".into())
            .spawn(move || {
                let mut reader = match repo.log() {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send((Err(e), true));
                        ctx.request_repaint();
                        return;
                    }
                };
                while let Ok(n) = want_rx.recv() {
                    let page = reader.next_page(n);
                    let done = page.is_err() || reader.is_done();
                    if tx.send((page, done)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    if done {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Loader { want, pages })
    }
}

/// The loaded part of the history and its layout.
pub(crate) struct History {
    loader: Option<Loader>,
    pending: bool,
    done: bool,
    commits: Vec<Commit>,
    rows: Vec<GraphRow>,
    graph: Graph<Oid>,
    index: HashMap<Oid, usize>,
    width: usize,
    error: Option<GitError>,
    /// Display row to bring into view at the next frame.
    scroll_to: Option<usize>,
}

impl History {
    /// No history (no repository open).
    pub(crate) fn empty() -> History {
        History {
            loader: None,
            pending: false,
            done: true,
            commits: Vec::new(),
            rows: Vec::new(),
            graph: Graph::new(),
            index: HashMap::new(),
            width: 1,
            error: None,
            scroll_to: None,
        }
    }

    /// Starts reading the history of `repo` in the background.
    pub(crate) fn start(repo: &Reader, ctx: &egui::Context) -> History {
        let mut h = History::empty();
        match Loader::start(repo.clone(), ctx.clone()) {
            Ok(loader) => {
                h.loader = Some(loader);
                h.done = false;
                h.request();
            }
            Err(e) => h.error = Some(GitError::Spawn(e)),
        }
        h
    }

    /// A history from commits already read (tests, small repositories).
    #[cfg(test)]
    pub(crate) fn from_commits(commits: Vec<Commit>) -> History {
        let mut h = History::empty();
        h.append(commits);
        h
    }

    fn append(&mut self, page: Vec<Commit>) {
        for c in page {
            let row = self.graph.push(&c.id, &c.parents);
            self.width = self.width.max(row.width);
            self.index.insert(c.id.clone(), self.commits.len());
            self.rows.push(row);
            self.commits.push(c);
        }
    }

    /// Asks for the next page unless one is on its way or the history is complete.
    fn request(&mut self) {
        if self.pending || self.done {
            return;
        }
        if let Some(loader) = &self.loader
            && loader.want.send(PAGE).is_ok()
        {
            self.pending = true;
        }
    }

    /// Takes pages that arrived.
    pub(crate) fn poll(&mut self) {
        let Some(loader) = &self.loader else {
            return;
        };
        let arrived: Vec<_> = loader.pages.try_iter().collect();
        for (page, done) in arrived {
            self.pending = false;
            match page {
                Ok(page) => self.append(page),
                Err(e) => self.error = Some(e),
            }
            if done {
                self.done = true;
                self.loader = None;
            }
        }
    }

    pub(crate) fn commits(&self) -> &[Commit] {
        &self.commits
    }

    pub(crate) fn is_loading(&self) -> bool {
        !self.done
    }

    pub(crate) fn error(&self) -> Option<&GitError> {
        self.error.as_ref()
    }

    pub(crate) fn position(&self, id: &Oid) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// The lane colour index of a loaded commit (sidebar dots).
    pub(crate) fn color_of(&self, id: &Oid) -> Option<usize> {
        self.position(id)
            .and_then(|i| self.rows.get(i))
            .map(|r| r.color)
    }

    pub(crate) fn commit(&self, id: &Oid) -> Option<&Commit> {
        self.position(id).and_then(|i| self.commits.get(i))
    }

    /// Asks for the next page (e.g. while looking for further search matches).
    pub(crate) fn load_more(&mut self) {
        self.request();
    }

    /// Brings display row `row` into view at the next frame.
    pub(crate) fn scroll_to(&mut self, row: usize) {
        self.scroll_to = Some(row);
    }
}

/// A ref label on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Label {
    pub text: String,
    pub kind: RefKind,
    pub head: bool,
}

/// Ref labels by commit.
#[derive(Debug, Clone, Default)]
pub(crate) struct Labels(HashMap<Oid, Vec<Label>>);

impl Labels {
    pub(crate) fn new(refs: &[Ref]) -> Labels {
        let mut map: HashMap<Oid, Vec<Label>> = HashMap::new();
        for r in refs {
            map.entry(r.target.clone()).or_default().push(Label {
                text: escape(&r.name),
                kind: r.kind,
                head: r.is_head,
            });
        }
        // The current branch first, then local, remote, tags.
        for labels in map.values_mut() {
            labels.sort_by_key(|l| (!l.head, l.kind));
        }
        Labels(map)
    }

    pub(crate) fn of(&self, id: &Oid) -> &[Label] {
        self.0.get(id).map_or(&[], Vec::as_slice)
    }
}

/// What is selected in the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Selection {
    None,
    WorkInProgress,
    Commit(Oid),
}

/// Counts shown on the work-in-progress row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WipCounts {
    pub changed: usize,
    pub staged: usize,
    pub conflicts: usize,
}

/// Everything the history view needs besides the history itself.
pub(crate) struct View<'a> {
    pub labels: &'a Labels,
    pub avatars: &'a crate::avatars::Avatars,
    /// An active search dims the commits that do not match.
    pub search: Option<&'a crate::search::Search>,
    pub wip: Option<WipCounts>,
    pub head: Option<&'a Oid>,
    pub now: i64,
    /// Commits where a shallow repository's history is cut.
    pub shallow: &'a [Oid],
}

/// What the user asked for in the history.
pub(crate) enum Action {
    Select(Selection),
    CopyId(Oid),
    CopyMessage(String),
    /// "Branch hier anlegen …"
    NewBranch(Oid),
    /// "Diesen Commit auschecken …" (detached HEAD)
    Detach(Oid),
    /// "Tag hier anlegen …"
    NewTag(Oid),
}

impl History {
    /// Number of display rows (work in progress included).
    pub(crate) fn display_rows(&self, wip: bool) -> usize {
        self.commits.len() + usize::from(wip)
    }

    /// Selection at display row `row`.
    pub(crate) fn selection_at(&self, row: usize, wip: bool) -> Selection {
        match (wip, row) {
            (true, 0) => Selection::WorkInProgress,
            (true, n) => self
                .commits
                .get(n - 1)
                .map_or(Selection::None, |c| Selection::Commit(c.id.clone())),
            (false, n) => self
                .commits
                .get(n)
                .map_or(Selection::None, |c| Selection::Commit(c.id.clone())),
        }
    }

    /// Display row of a selection.
    pub(crate) fn row_of(&self, selection: &Selection, wip: bool) -> Option<usize> {
        match selection {
            Selection::None => None,
            Selection::WorkInProgress => wip.then_some(0),
            Selection::Commit(id) => self.position(id).map(|i| i + usize::from(wip)),
        }
    }

    /// Draws the history table; returns what the user did.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        view: &View<'_>,
        selection: &Selection,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let wip = view.wip.is_some();
        let total = self.display_rows(wip);
        let head_row = view.head.and_then(|h| self.position(h));
        // At least wide enough for the column title.
        let graph_w = ((self.width.clamp(1, MAX_LANES) as f32) * LANE_W + 8.0).max(64.0);
        paint_header(ui, t, graph_w);
        let mut area = egui::ScrollArea::vertical()
            .id_salt("history")
            .auto_shrink([false, false]);
        if let Some(row) = self.scroll_to.take() {
            let visible = ui.available_height();
            let target = (row as f32 * ROW_H - visible / 2.0 + ROW_H).max(0.0);
            area = area.vertical_scroll_offset(target);
        }
        let mut need_more = false;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            area.show_rows(ui, ROW_H, total, |ui, range| {
                if range.end + 200 > total {
                    need_more = true;
                }
                for row in range {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                    let sel = self.selection_at(row, wip);
                    let selected = &sel == selection;
                    if selected {
                        ui.painter().rect_filled(rect, 0.0, t.selection);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, t.row_alt);
                    }
                    let cols = Columns::new(rect, graph_w);
                    let label = match (&sel, view.wip) {
                        (Selection::WorkInProgress, Some(counts)) => {
                            self.paint_wip(ui, t, &cols, counts, head_row);
                            wip_text(counts)
                        }
                        (Selection::Commit(_), _) => {
                            let i = row - usize::from(wip);
                            let first_below_wip = wip && i == 0 && head_row == Some(0);
                            self.paint_commit(ui, t, &cols, view, i, first_below_wip, head_row)
                        }
                        _ => String::new(),
                    };
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::Button, true, selected, &label)
                    });
                    if resp.clicked() {
                        actions.push(Action::Select(sel.clone()));
                    }
                    if let Selection::Commit(id) = &sel {
                        let message = self
                            .commit(id)
                            .map(|c| c.subject.clone())
                            .unwrap_or_default();
                        resp.context_menu(|ui| {
                            if ui.button(tr("Commit-ID kopieren")).clicked() {
                                actions.push(Action::CopyId(id.clone()));
                                ui.close();
                            }
                            if ui.button(tr("Nachricht kopieren")).clicked() {
                                actions.push(Action::CopyMessage(message.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(tr("Branch hier anlegen …")).clicked() {
                                actions.push(Action::NewBranch(id.clone()));
                                ui.close();
                            }
                            if ui.button(tr("Tag hier anlegen …")).clicked() {
                                actions.push(Action::NewTag(id.clone()));
                                ui.close();
                            }
                            if ui.button(tr("Diesen Commit auschecken …")).clicked() {
                                actions.push(Action::Detach(id.clone()));
                                ui.close();
                            }
                        });
                    }
                }
            });
        });
        if need_more {
            self.request();
        }
        actions
    }

    fn paint_wip(
        &self,
        ui: &egui::Ui,
        t: &Tokens,
        cols: &Columns,
        counts: WipCounts,
        head_row: Option<usize>,
    ) {
        let p = ui.painter();
        let lane = match head_row {
            Some(0) => self.rows.first().map_or(0, |r| r.lane),
            _ => 0,
        };
        let c = cols.lane_center(lane);
        let stroke = Stroke::new(2.0, t.text_dim);
        if head_row == Some(0) {
            p.extend(Shape::dashed_line(
                &[pos2(c.x, c.y + NODE_R + 1.0), pos2(c.x, cols.rect.bottom())],
                stroke,
                3.0,
                3.0,
            ));
        }
        let ring: Vec<Pos2> = (0..=24)
            .map(|k| {
                let a = k as f32 / 24.0 * std::f32::consts::TAU;
                c + vec2(a.cos(), a.sin()) * NODE_R
            })
            .collect();
        p.extend(Shape::dashed_line(&ring, stroke, 2.5, 2.0));
        let text = wip_text(counts);
        p.with_clip_rect(cols.message).text(
            pos2(cols.message.left(), c.y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(14.0),
            t.text_dim,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_commit(
        &self,
        ui: &egui::Ui,
        t: &Tokens,
        cols: &Columns,
        view: &View<'_>,
        i: usize,
        below_wip: bool,
        head_row: Option<usize>,
    ) -> String {
        let (Some(commit), Some(row)) = (self.commits.get(i), self.rows.get(i)) else {
            return String::new();
        };
        let p = ui.painter();
        let lane_color = |c: usize| t.lanes[c % t.lanes.len()];
        let mid = cols.rect.center().y;
        let graph = p.with_clip_rect(cols.graph);
        for s in &row.top {
            line(
                &graph,
                cols.lane_x(s.from),
                cols.rect.top(),
                cols.lane_x(s.to),
                mid,
                lane_color(s.color),
            );
        }
        for s in &row.bottom {
            line(
                &graph,
                cols.lane_x(s.from),
                mid,
                cols.lane_x(s.to),
                cols.rect.bottom(),
                lane_color(s.color),
            );
        }
        let c = cols.lane_center(row.lane);
        if below_wip {
            graph.extend(Shape::dashed_line(
                &[pos2(c.x, cols.rect.top()), pos2(c.x, c.y - NODE_R - 1.0)],
                Stroke::new(2.0, t.text_dim),
                3.0,
                3.0,
            ));
        }
        let cut = view.shallow.contains(&commit.id);
        if cut {
            // History goes on, but this clone does not have it.
            graph.extend(Shape::dashed_line(
                &[pos2(c.x, c.y + NODE_R + 1.0), pos2(c.x, cols.rect.bottom())],
                Stroke::new(1.5, t.text_dim),
                2.0,
                3.0,
            ));
        }
        let color = lane_color(row.color);
        let labels = view.labels.of(&commit.id);
        // Refs column, joined to the node by a line in the lane colour.
        if let Some(end) = paint_labels(ui, t, cols.refs, labels, color) {
            p.with_clip_rect(cols.rect).line_segment(
                [pos2(end + 2.0, mid), pos2(c.x - NODE_R, mid)],
                Stroke::new(1.5, color),
            );
        }
        let node = p.with_clip_rect(cols.graph.expand2(vec2(0.0, 2.0)));
        if commit.parents.len() > 1 {
            // Merge commits as small dots, so the commits with content stand out.
            node.circle_filled(c, 4.5, color);
        } else if let Some(tex) = view.avatars.get(ui.ctx(), &commit.author.email) {
            let disc = Rect::from_center_size(c, vec2(NODE_R * 2.0, NODE_R * 2.0));
            egui::Image::from_texture((tex, disc.size()))
                .corner_radius(NODE_R)
                .paint_at(ui, disc);
            node.circle_stroke(c, NODE_R, Stroke::new(2.0, color));
        } else {
            node.circle_filled(c, NODE_R, color);
            node.text(
                c,
                egui::Align2::CENTER_CENTER,
                initials(&commit.author.name),
                FontId::proportional(9.5),
                t.on_lane,
            );
        }
        if head_row == Some(i) {
            node.circle_stroke(c, NODE_R + 3.0, Stroke::new(1.5, t.text_strong));
        }

        // Message, author, date.
        let subject = escape(&commit.subject);
        let shown = p.with_clip_rect(cols.message).text(
            pos2(cols.message.left(), mid),
            egui::Align2::LEFT_CENTER,
            &subject,
            FontId::proportional(14.0),
            t.text,
        );
        if cut {
            p.with_clip_rect(cols.message).text(
                pos2(shown.right() + 12.0, mid),
                egui::Align2::LEFT_CENTER,
                tr("Historie hier abgeschnitten"),
                FontId::proportional(12.5),
                t.text_dim,
            );
        }
        if cols.author.width() > 0.0 {
            p.with_clip_rect(cols.author).text(
                pos2(cols.author.left(), mid),
                egui::Align2::LEFT_CENTER,
                escape(&commit.author.name),
                FontId::proportional(13.0),
                t.text_dim,
            );
        }
        let date = relative_date(view.now, commit.author.time);
        p.text(
            pos2(cols.date.right(), mid),
            egui::Align2::RIGHT_CENTER,
            &date,
            FontId::proportional(13.0),
            t.text_dim,
        );
        if let Some(search) = view.search.filter(|s| s.active())
            && !search.is_match(commit)
        {
            p.rect_filled(cols.rect, 0.0, t.panel_bg.gamma_multiply(0.7));
        }
        let refs: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        let mut text = subject;
        if !refs.is_empty() {
            text = format!("{text} ({})", refs.join(", "));
        }
        if cut {
            text = format!("{text} ({})", tr("Historie hier abgeschnitten"));
        }
        format!("{text}, {}, {date}", escape(&commit.author.name))
    }
}

/// Where each column of a row lies.
struct Columns {
    rect: Rect,
    refs: Rect,
    graph: Rect,
    message: Rect,
    author: Rect,
    date: Rect,
}

impl Columns {
    fn new(rect: Rect, graph_w: f32) -> Columns {
        let refs = Rect::from_min_size(rect.min, vec2(REFS_W, rect.height()));
        let graph =
            Rect::from_min_size(pos2(refs.right(), rect.top()), vec2(graph_w, rect.height()));
        let date = Rect::from_min_max(
            pos2(rect.right() - DATE_W - 8.0, rect.top()),
            pos2(rect.right() - 8.0, rect.bottom()),
        );
        let author_w = if rect.width() >= AUTHOR_MIN_ROW {
            AUTHOR_W
        } else {
            0.0
        };
        let author = Rect::from_min_max(
            pos2(date.left() - 8.0 - author_w, rect.top()),
            pos2(date.left() - 8.0, rect.bottom()),
        );
        let message = Rect::from_min_max(
            pos2(graph.right() + 4.0, rect.top()),
            pos2(
                (author.left() - 8.0).max(graph.right() + 4.0),
                rect.bottom(),
            ),
        );
        Columns {
            rect,
            refs,
            graph,
            message,
            author,
            date,
        }
    }

    fn lane_x(&self, lane: usize) -> f32 {
        self.graph.left() + 4.0 + (lane as f32 + 0.5) * LANE_W
    }

    fn lane_center(&self, lane: usize) -> Pos2 {
        pos2(self.lane_x(lane), self.rect.center().y)
    }
}

/// A line of the graph from (x0, y0) to (x1, y1): straight when it stays in its lane, an S-curve
/// when it changes lanes.
fn line(p: &egui::Painter, x0: f32, y0: f32, x1: f32, y1: f32, color: Color32) {
    let stroke = Stroke::new(2.0, color);
    if (x0 - x1).abs() < 0.5 {
        p.line_segment([pos2(x0, y0), pos2(x1, y1)], stroke);
    } else {
        let ym = (y0 + y1) / 2.0;
        p.add(CubicBezierShape::from_points_stroke(
            [pos2(x0, y0), pos2(x0, ym), pos2(x1, ym), pos2(x1, y1)],
            false,
            Color32::TRANSPARENT,
            stroke,
        ));
    }
}

/// Pills for the refs on a commit: local branches filled in the lane colour, remote branches
/// outlined, tags as small boxes; what does not fit becomes `+n`.
/// Returns where the last drawn label ends, for the line to the node.
fn paint_labels(
    ui: &egui::Ui,
    t: &Tokens,
    area: Rect,
    labels: &[Label],
    color: Color32,
) -> Option<f32> {
    if labels.is_empty() {
        return None;
    }
    let p = ui.painter().with_clip_rect(area);
    let font = FontId::proportional(12.0);
    let mut x = area.left() + 6.0;
    let y = area.center().y;
    for (n, l) in labels.iter().enumerate() {
        let text_color = match l.kind {
            RefKind::Local => t.on_lane,
            _ => t.text,
        };
        let galley = p.layout_no_wrap(l.text.clone(), font.clone(), text_color);
        let w = galley.size().x + 12.0;
        let rest = labels.len() - n;
        let reserve = if rest > 1 { 28.0 } else { 0.0 };
        if x + w > area.right() - 6.0 - reserve {
            let more = format!("+{rest}");
            let r = p.text(
                pos2(x, y),
                egui::Align2::LEFT_CENTER,
                more,
                font,
                t.text_dim,
            );
            return Some(r.right());
        }
        let pill = Rect::from_min_size(pos2(x, y - 9.0), vec2(w, 18.0));
        match l.kind {
            RefKind::Local => {
                p.rect_filled(pill, 9.0, color);
                if l.head {
                    p.rect_stroke(
                        pill,
                        9.0,
                        Stroke::new(1.5, t.text_strong),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            RefKind::Remote => {
                p.rect_stroke(pill, 9.0, Stroke::new(1.5, color), egui::StrokeKind::Inside);
            }
            RefKind::Tag => {
                p.rect_filled(pill, 3.0, t.button);
                p.rect_stroke(
                    pill,
                    3.0,
                    Stroke::new(1.0, t.border_light),
                    egui::StrokeKind::Inside,
                );
            }
        }
        p.galley(pos2(x + 6.0, y - galley.size().y / 2.0), galley, text_color);
        x += w + 4.0;
    }
    Some(x - 4.0)
}

/// Column titles above the rows (they stay while the rows scroll).
fn paint_header(ui: &mut egui::Ui, t: &Tokens, graph_w: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let cols = Columns::new(rect, graph_w);
    let p = ui.painter();
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, t.border),
    );
    let font = FontId::proportional(11.5);
    for (area, text, align) in [
        (cols.refs, tr("Branch / Tag"), egui::Align2::LEFT_CENTER),
        (cols.graph, tr("Graph"), egui::Align2::LEFT_CENTER),
        (
            cols.message,
            tr("Commit-Nachricht"),
            egui::Align2::LEFT_CENTER,
        ),
        (cols.author, tr("Autor"), egui::Align2::LEFT_CENTER),
        (cols.date, tr("Datum"), egui::Align2::RIGHT_CENTER),
    ] {
        if area.width() <= 0.0 {
            continue;
        }
        let x = if align == egui::Align2::RIGHT_CENTER {
            area.right()
        } else {
            area.left() + 6.0
        };
        p.with_clip_rect(area).text(
            pos2(x, area.center().y),
            align,
            text.to_uppercase(),
            font.clone(),
            t.text_dim,
        );
    }
}

fn wip_text(c: WipCounts) -> String {
    let mut text = trf(
        "Arbeitsstand: {geaendert} geändert, {vorgemerkt} vorgemerkt",
        &[("geaendert", &c.changed), ("vorgemerkt", &c.staged)],
    );
    if c.conflicts > 0 {
        text.push_str(&trf(", {n} Konflikte", &[("n", &c.conflicts)]));
    }
    text
}

/// Up to two initials of a name, upper case.
pub(crate) fn initials(name: &str) -> String {
    // Letters only: control and formatting characters are not alphanumeric and drop out.
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".into()
    } else {
        letters
    }
}

/// "jetzt", "5 Min.", "3 Std.", "2 Tg.", "4 Mon.", "2 J." (and the English forms).
pub(crate) fn relative_date(now: i64, then: i64) -> String {
    let d = now.saturating_sub(then).max(0);
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    if d < MIN {
        tr("jetzt").to_owned()
    } else if d < HOUR {
        trf("{n} Min.", &[("n", &(d / MIN))])
    } else if d < DAY {
        trf("{n} Std.", &[("n", &(d / HOUR))])
    } else if d < 30 * DAY {
        trf("{n} Tg.", &[("n", &(d / DAY))])
    } else if d < 365 * DAY {
        trf("{n} Mon.", &[("n", &(d / (30 * DAY)))])
    } else {
        trf("{n} J.", &[("n", &(d / (365 * DAY)))])
    }
}

/// Seconds since the Unix epoch as `YYYY-MM-DD HH:MM UTC`.
pub(crate) fn absolute_date(t: i64) -> String {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        (secs % 3600) / 60
    )
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(absolute_date(0), "1970-01-01 00:00 UTC");
        assert_eq!(absolute_date(1_700_000_000), "2023-11-14 22:13 UTC");
        assert_eq!(absolute_date(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(absolute_date(-86_400), "1969-12-31 00:00 UTC");
        assert_eq!(relative_date(1000, 990), "jetzt");
        assert_eq!(relative_date(10_000, 10_000 - 3 * 3600), "3 Std.");
        assert_eq!(
            relative_date(0, 100),
            "jetzt",
            "future dates do not go negative"
        );
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Ann Bee Cee"), "AB");
        assert_eq!(initials("zoë"), "Z");
        assert_eq!(initials(""), "?");
        assert_eq!(
            initials("  \u{202e}x"),
            "X",
            "formatting characters drop out"
        );
    }
}
