//! The Mail view's label browser: a control beside the mailbox switcher that names the label on screen, and the
//! popover it opens over the view, with a filter field and the user's labels as a tree (`mail_logic::label_rows`):
//! each with its color, its name, and its unread count, parents folding what is under them. The arrows move the pick,
//! Enter or a click shows that label's threads, Escape closes it; `keys::MAIL_LABELS` opens it from the Mail view.
//! While it is open the emails' webviews hide (`MailView::apply_cover`), since they are native views drawn over it.

use gchat_core::Mailbox;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::calendar::TOOLBAR_HEIGHT;
use crate::keys;
use crate::mail::MailView;
use crate::mail_logic::{LabelRow, label_rows, leaf_name, step};
use crate::theme::{
  ACCENT, ACCENT_BG, BORDER, MUTED, POPOVER, ROW_HOVER, ROW_SELECTED, TEXT, control_fill, data_color, rgb,
};
use crate::ui::{Icon, count_badge, icon};

/// Width of the popover.
const POPOVER_WIDTH: f32 = 320.0;
/// The tallest the popover gets; its list scrolls past that.
const POPOVER_MAX_HEIGHT: f32 = 440.0;
/// Height of a label's row.
const LABEL_ROW_HEIGHT: f32 = 30.0;
/// How far each level of the tree is indented.
const INDENT: f32 = 16.0;

impl MailView {
  /// The rows the popover lists: the tree, or what the filter field's text matches.
  fn shown_labels(&self, cx: &App) -> Vec<LabelRow> {
    let filter = self.label_filter.read(cx).value();
    label_rows(&self.store.read(cx).mail.labels, &filter, &self.label_folded)
  }

  /// Opens the popover with an empty filter that has the keyboard. Nothing opens without labels.
  pub(crate) fn open_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.store.read(cx).mail.labels.is_empty() {
      return;
    }
    self.labels_open = true;
    self.label_pick = None;
    self.label_filter.update(cx, |state, cx| {
      state.set_value("", window, cx);
      state.focus(window, cx);
    });
    self.apply_cover(cx);
    cx.notify();
  }

  /// Closes the popover and gives the keyboard back to the view. Answers whether it was open.
  pub(crate) fn close_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if !self.labels_open {
      return false;
    }
    self.labels_open = false;
    window.focus(&self.focus, cx);
    self.apply_cover(cx);
    cx.notify();
    true
  }

  /// The control's click and the key: opens the popover, or closes it when it is open.
  pub(crate) fn toggle_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.close_labels(window, cx) {
      self.open_labels(window, cx);
    }
  }

  /// Moves the pick one row down or up.
  fn step_label(&mut self, forward: bool, cx: &mut Context<Self>) {
    let rows = self.shown_labels(cx).len();
    self.label_pick = step(self.label_pick, rows, forward);
    cx.notify();
  }

  /// Shows the threads of row `ix`, or else of the row picked, or else of the first: the popover closes, and the
  /// label is picked as a mailbox of the switcher is.
  pub(crate) fn pick_label(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
    let rows = self.shown_labels(cx);
    let Some(row) = ix.or(self.label_pick).or(Some(0)).and_then(|ix| rows.get(ix)) else {
      return;
    };
    let id = row.id.clone();
    self.close_labels(window, cx);
    self.pick_mailbox(Mailbox::Label(id), window, cx);
  }

  /// Folds the labels under a parent, or shows them again.
  fn toggle_fold(&mut self, id: &str, cx: &mut Context<Self>) {
    if !self.label_folded.remove(id) {
      self.label_folded.insert(id.to_string());
    }
    self.label_pick = None;
    cx.notify();
  }

  /// The control beside the switcher: `Labels`, or the label on screen by its own name, selected as the switcher's
  /// entries are, with its whole path in the tooltip. Absent while the account has no labels.
  pub(crate) fn labels_control(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
    let labels = &self.store.read(cx).mail.labels;
    if labels.is_empty() {
      return None;
    }
    let picked = match &self.mailbox {
      Mailbox::Label(id) => labels.iter().find(|label| label.id == *id),
      _ => None,
    };
    let selected = picked.is_some() || self.labels_open;
    let name = picked.map_or_else(|| "Labels".to_string(), |label| leaf_name(&label.name).to_string());
    let path = picked.map_or("Labels", |label| label.name.as_str());
    let tip: SharedString = format!("{path} ({})", keys::text(keys::MAIL_LABELS)).into();
    Some(
      h_flex()
        .id("mail-labels")
        .flex_shrink_0()
        .max_w(px(200.))
        .h(px(30.))
        .px_3()
        .gap_1p5()
        .items_center()
        .rounded(px(9.))
        .text_size(px(12.))
        .text_color(rgb(if selected { TEXT } else { MUTED }))
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
          rgb(ACCENT)
        } else {
          transparent_black().into()
        })
        .bg(if selected {
          rgb(ACCENT_BG)
        } else {
          control_fill()
        })
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(cx.listener(|this, _, window, cx| this.toggle_labels(window, cx)))
        .child(icon(Icon::Tag, 13., if selected { TEXT } else { MUTED }))
        .child(div().min_w_0().truncate().child(name))
        .child(icon(Icon::ChevronDown, 12., MUTED)),
    )
  }

  /// The popover, over the view under the toolbar, while it is open: a press outside it closes it.
  pub(crate) fn labels_popover(&self, cx: &Context<Self>) -> Option<AnyElement> {
    if !self.labels_open {
      return None;
    }
    let rows = self.shown_labels(cx);
    let filtering = !self.label_filter.read(cx).value().trim().is_empty();
    let list: AnyElement = if rows.is_empty() {
      div()
        .px_3()
        .py_2()
        .text_size(px(13.))
        .text_color(rgb(MUTED))
        .child("No label matches")
        .into_any_element()
    } else {
      v_flex()
        .children(rows.iter().enumerate().map(|(ix, row)| self.label_row(ix, row, filtering, cx)))
        .into_any_element()
    };
    let field = h_flex()
      .flex_shrink_0()
      .h(px(32.))
      .px_2p5()
      .gap_2()
      .items_center()
      .rounded(px(8.))
      .bg(control_fill())
      .child(icon(Icon::Search, 14., MUTED))
      .child(
        div()
          .flex_1()
          .child(Input::new(&self.label_filter).appearance(false).bordered(false)),
      );
    let panel = v_flex()
      .id("mail-labels-popover")
      .absolute()
      .top(px(TOOLBAR_HEIGHT - 6.))
      .left(px(16.))
      .w(px(POPOVER_WIDTH))
      .max_h(px(POPOVER_MAX_HEIGHT))
      .p_1p5()
      .gap_1p5()
      .rounded(px(12.))
      .border_1()
      .border_color(rgb(BORDER))
      .bg(rgb(POPOVER))
      .shadow_lg()
      .occlude()
      // The field has the keyboard: the arrows move the pick from it.
      .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
        let forward = match event.keystroke.key.as_str() {
          "down" => true,
          "up" => false,
          _ => return,
        };
        this.step_label(forward, cx);
        cx.stop_propagation();
      }))
      .child(field)
      .child(div().id("mail-labels-list").flex_1().min_h_0().overflow_y_scroll().child(list));
    let backdrop = div()
      .id("mail-labels-backdrop")
      .absolute()
      .top_0()
      .left_0()
      .size_full()
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, _, window, cx| {
          this.close_labels(window, cx);
        }),
      );
    Some(
      div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(backdrop)
        .child(panel)
        .into_any_element(),
    )
  }

  /// One label: its fold toggle when it is a parent in the tree, its color, its name (bold while it has unread
  /// threads), and the count of those.
  fn label_row(&self, ix: usize, row: &LabelRow, filtering: bool, cx: &Context<Self>) -> AnyElement {
    let picked = self.label_pick == Some(ix);
    let unread = row.unread > 0;
    let fold: AnyElement = if row.parent && !filtering {
      let id = row.id.clone();
      div()
        .id(("mail-label-fold", ix))
        .size(px(16.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .hover(|s| s.bg(rgb(ROW_HOVER)))
        .on_click(cx.listener(move |this, _, _, cx| {
          cx.stop_propagation();
          this.toggle_fold(&id, cx);
        }))
        .child(icon(
          if row.folded {
            Icon::ChevronRight
          } else {
            Icon::ChevronDown
          },
          12.,
          MUTED,
        ))
        .into_any_element()
    } else {
      div().w(px(16.)).flex_shrink_0().into_any_element()
    };
    let dot = div().size(px(8.)).flex_shrink_0().rounded_full().map(|dot| match row.color {
      Some(color) => dot.bg(data_color(color)),
      None => dot.border_1().border_color(rgb(MUTED)),
    });
    h_flex()
      .id(("mail-label", ix))
      .h(px(LABEL_ROW_HEIGHT))
      .flex_shrink_0()
      .pl(px(INDENT.mul_add(row.depth as f32, 4.)))
      .pr_2()
      .gap_2()
      .items_center()
      .rounded(px(8.))
      .cursor_pointer()
      .when(picked, |row| row.bg(rgb(ROW_SELECTED)))
      .when(!picked, |row| row.hover(|s| s.bg(rgb(ROW_HOVER))))
      .on_click(cx.listener(move |this, _, window, cx| this.pick_label(Some(ix), window, cx)))
      .child(fold)
      .child(dot)
      .child(
        div()
          .flex_1()
          .min_w_0()
          .truncate()
          .text_size(px(13.))
          .text_color(rgb(TEXT))
          .when(unread, |name| name.font_weight(FontWeight::SEMIBOLD))
          .child(row.name.clone()),
      )
      .when(unread, |line| line.child(count_badge(row.unread as usize)))
      .into_any_element()
  }
}
