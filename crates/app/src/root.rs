//! Window root: title bar with search and status, sidebar, content panel, switcher and theme picker overlays.

use gchat_core::{Connection, Core};
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use std::sync::Arc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::emoji_picker::{EmojiHandle, EmojiPicker};
use crate::home::HomeView;
use crate::menu::{ContextMenu, MenuHandle};
use crate::new_chat::{NewChat, NewChatEvent, OpenNewChat};
use crate::settings;
use crate::settings_view::{SettingsEvent, SettingsView};
use crate::shortcut::ShortcutView;
use crate::sidebar::Sidebar;
use crate::status;
use crate::status_menu::{self, ComposerHandle, OpenNotificationSettings, StatusComposer};
use crate::store::{Store, View};
use crate::switcher::{Switcher, SwitcherEvent};
use crate::theme::{BG, FIELD, MUTED, PANEL, ROW_HOVER, TEXT, is_light, rgb};
use crate::theme_picker::{ThemePicker, ThemePickerEvent};
use crate::ui::{Icon, icon};
use crate::viewer::{Viewer, ViewerHandle};
use crate::welcome::WelcomeView;

actions!(
  working,
  [
    OpenSwitcher,
    OpenThemePicker,
    OpenSettings,
    GoHome,
    Quit,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    RewriteDraft,
    FindInChat,
    FindNext,
    FindPrevious,
    GoBack,
    GoForward
  ]
);

/// Height of the top bar. The traffic lights are centered in it (see `main.rs`).
pub const TOP_BAR_HEIGHT: f32 = 52.0;

/// The application menu in the macOS menu bar.
pub fn menus() -> Vec<Menu> {
  vec![
    Menu {
      name: "Working".into(),
      disabled: false,
      items: vec![
        MenuItem::action("Settings...", OpenSettings),
        MenuItem::action("Color Theme…", OpenThemePicker),
        MenuItem::separator(),
        MenuItem::action("Hide Working", Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        MenuItem::action("Quit Working", Quit),
      ],
    },
    Menu {
      name: "Edit".into(),
      disabled: false,
      items: vec![
        MenuItem::os_action("Undo", Undo, OsAction::Undo),
        MenuItem::os_action("Redo", Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action("Cut", Cut, OsAction::Cut),
        MenuItem::os_action("Copy", Copy, OsAction::Copy),
        MenuItem::os_action("Paste", Paste, OsAction::Paste),
        MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        MenuItem::separator(),
        MenuItem::action("Find…", FindInChat),
        MenuItem::action("Find Next", FindNext),
        MenuItem::action("Find Previous", FindPrevious),
        MenuItem::separator(),
        MenuItem::action("Rewrite", RewriteDraft),
      ],
    },
    Menu {
      name: "Go".into(),
      disabled: false,
      items: vec![
        MenuItem::action("Back", GoBack),
        MenuItem::action("Forward", GoForward),
        MenuItem::separator(),
        MenuItem::action("Home", GoHome),
        MenuItem::action("New Chat…", OpenNewChat),
        MenuItem::action("Quick Switcher…", OpenSwitcher),
      ],
    },
    Menu {
      name: "Window".into(),
      disabled: false,
      items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)],
    },
  ]
}

pub struct RootView {
  store: Entity<Store>,
  sidebar: Entity<Sidebar>,
  home: Entity<HomeView>,
  shortcuts: Entity<ShortcutView>,
  search: Entity<InputState>,
  focus: FocusHandle,
  /// The top-bar mark, for the dark look and for the light one.
  logo: [Arc<Image>; 2],
  viewer: Entity<Viewer>,
  menu: Entity<ContextMenu>,
  emoji: Entity<EmojiPicker>,
  composer: Entity<StatusComposer>,
  switcher: Option<(Entity<Switcher>, Subscription)>,
  theme_picker: Option<(Entity<ThemePicker>, Subscription)>,
  settings: Option<(Entity<SettingsView>, Subscription)>,
  new_chat: Option<(Entity<NewChat>, Subscription)>,
  welcome: Option<Entity<WelcomeView>>,
  _subs: Vec<Subscription>,
  _pump: Task<()>,
  _opened: Task<()>,
}

impl RootView {
  pub fn new(core: Core, store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let sidebar = cx.new(|_| Sidebar::new(store.clone()));
    let home = cx.new(|_| HomeView::new(store.clone()));
    let shortcuts = cx.new(|_| ShortcutView::new(store.clone()));
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search chat"));
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    let images = store.read(cx).images.clone();
    let viewer = cx.new(|cx| Viewer::new(core.clone(), images, cx));
    let menu = cx.new(ContextMenu::new);
    let emoji = cx.new(|cx| EmojiPicker::new(window, cx));
    let composer = cx.new(|cx| StatusComposer::new(core.clone(), window, cx));
    cx.set_global(ViewerHandle(viewer.clone()));
    cx.set_global(MenuHandle(menu.clone()));
    cx.set_global(EmojiHandle(emoji.clone()));
    cx.set_global(ComposerHandle(composer.clone()));

    let subs = vec![
      cx.subscribe_in(&search, window, |this, search, event: &InputEvent, _, cx| match event {
        InputEvent::Change => {
          let text = search.read(cx).value().to_string();
          this.store.update(cx, |store, cx| store.set_query(text, cx));
        },
        // Typing filters conversation names; Enter searches messages.
        InputEvent::PressEnter { .. } => this.store.update(cx, super::store::Store::run_search),
        _ => {},
      }),
      // Opening a conversation clears the store's query; mirror that in the field.
      cx.observe_in(&store, window, |this, store, window, cx| {
        if store.read(cx).query.is_empty() && !this.search.read(cx).value().is_empty() {
          this.search.update(cx, |state, cx| state.set_value("", window, cx));
        }
      }),
      // "System" appearance follows macOS as it changes.
      cx.observe_window_appearance(window, |_, _, cx| settings::apply_appearance(cx)),
      cx.observe_global::<settings::Settings>(|_, cx| cx.notify()),
      cx.observe_global::<status::Mine>(|_, cx| cx.notify()),
    ];

    // One task drains the core: everything available, then a single state update.
    let events = core.events().clone();
    let target = store.clone();
    let pump = cx.spawn_in(window, async move |_, cx| {
      while let Ok(first) = events.recv_async().await {
        let mut batch = vec![first];
        while let Ok(event) = events.try_recv() {
          batch.push(event);
        }
        let target = target.clone();
        if cx
          .update(|window, cx| target.update(cx, |store, cx| store.apply(batch, window, cx)))
          .is_err()
        {
          break;
        }
      }
    });

    // A clicked notification opens its conversation.
    let clicks = crate::notify::start();
    let target = store.clone();
    let opened = cx.spawn_in(window, async move |_, cx| {
      while let Ok(key) = clicks.recv_async().await {
        let target = target.clone();
        let opening = cx.update(|window, cx| {
          target.update(cx, |store, cx| store.open_key(&key, window, cx));
          cx.activate(true);
          window.activate_window();
        });
        if opening.is_err() {
          break;
        }
      }
    });

    // The mark from logo/logo.svg, rendered by scripts/logo.sh well above the 24px it is shown at.
    let mark = |bytes: &[u8]| Arc::new(Image::from_bytes(ImageFormat::Png, bytes.to_vec()));
    let logo = [
      mark(include_bytes!("../assets/logo.png")),
      mark(include_bytes!("../assets/logo-light.png")),
    ];
    Self {
      store,
      sidebar,
      home,
      search,
      focus,
      shortcuts,
      logo,
      viewer,
      menu,
      emoji,
      composer,
      switcher: None,
      theme_picker: None,
      settings: None,
      new_chat: None,
      welcome: None,
      _subs: subs,
      _pump: pump,
      _opened: opened,
    }
  }

  fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.open_settings_at(false, window, cx);
  }

  fn open_settings_at(&mut self, notifications: bool, window: &mut Window, cx: &mut Context<Self>) {
    if self.settings.is_some() {
      return;
    }
    let core = self.store.read(cx).core.clone();
    let view = cx.new(|cx| {
      if notifications {
        SettingsView::at_notifications(core, window, cx)
      } else {
        SettingsView::new(core, window, cx)
      }
    });
    let sub = cx.subscribe_in(&view, window, |this, _, _: &SettingsEvent, _, cx| {
      this.settings = None;
      cx.notify();
    });
    self.settings = Some((view, sub));
    cx.notify();
  }

  fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.switcher.is_some() {
      return;
    }
    let store = self.store.clone();
    let switcher = cx.new(|cx| Switcher::new(store, window, cx));
    let sub = cx.subscribe_in(&switcher, window, |this, _, event: &SwitcherEvent, window, cx| {
      this.close_switcher(window, cx);
      if let SwitcherEvent::Pick(id) = event {
        let id = id.clone();
        this.store.update(cx, |store, cx| store.open(id, window, cx));
      }
    });
    self.switcher = Some((switcher, sub));
    cx.notify();
  }

  fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.theme_picker.is_some() {
      return;
    }
    let picker = cx.new(|cx| ThemePicker::new(window, cx));
    let sub = cx.subscribe_in(&picker, window, |this, _, event: &ThemePickerEvent, window, cx| {
      if let ThemePickerEvent::Pick(index) = *event {
        settings::Settings::update(cx, |settings| settings.pick_theme(index));
      }
      this.close_theme_picker(window, cx);
    });
    self.theme_picker = Some((picker, sub));
    cx.notify();
  }

  /// Closes the picker and shows the saved theme again, which is the picked one after a pick.
  fn close_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.theme_picker.take().is_none() {
      return;
    }
    settings::apply_appearance(cx);
    let conv = self.store.read(cx).conv.clone();
    match conv {
      Some(conv) => conv.update(cx, |conv, cx| conv.focus_composer(window, cx)),
      None => window.focus(&self.focus, cx),
    }
    cx.notify();
  }

  fn open_new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.new_chat.is_some() {
      self.close_new_chat(window, cx);
    }
    let store = self.store.clone();
    let view = cx.new(|cx| NewChat::new(store, window, cx));
    let sub = cx.subscribe_in(&view, window, |this, _, event: &NewChatEvent, window, cx| match event {
      NewChatEvent::Close => this.close_new_chat(window, cx),
      NewChatEvent::Open(id) => {
        let id = id.clone();
        this.close_new_chat(window, cx);
        this.store.update(cx, |store, cx| store.open(id, window, cx));
      },
    });
    self.new_chat = Some((view, sub));
    cx.notify();
  }

  fn close_new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.new_chat.take().is_none() {
      return;
    }
    let conv = self.store.read(cx).conv.clone();
    match conv {
      Some(conv) => conv.update(cx, |conv, cx| conv.focus_composer(window, cx)),
      None => window.focus(&self.focus, cx),
    }
    cx.notify();
  }

  fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.switcher = None;
    let conv = self.store.read(cx).conv.clone();
    match conv {
      Some(conv) => conv.update(cx, |conv, cx| conv.focus_composer(window, cx)),
      None => window.focus(&self.focus, cx),
    }
    cx.notify();
  }

  fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.switcher = None;
    self.new_chat = None;
    self.store.update(cx, super::store::Store::go_home);
    window.focus(&self.focus, cx);
  }

  fn on_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.composer.read(cx).is_open() && !self.emoji.read(cx).is_open() {
      self.composer.update(cx, |composer, cx| composer.close(window, cx));
    } else if self.emoji.read(cx).is_open() {
      self.emoji.update(cx, |emoji, cx| emoji.close(window, cx));
    } else if self.settings.take().is_some() {
      cx.notify();
    } else if self.menu.read(cx).is_open() {
      self.menu.update(cx, |menu, cx| menu.close(window, cx));
    } else if self.viewer.read(cx).is_open() {
      self.viewer.update(cx, |viewer, cx| viewer.close(window, cx));
    } else if self.new_chat.is_some() {
      self.close_new_chat(window, cx);
    } else if self.switcher.is_some() {
      self.close_switcher(window, cx);
    } else if self.theme_picker.is_some() {
      self.close_theme_picker(window, cx);
    } else if !self.store.read(cx).query.is_empty() {
      self.search.update(cx, |state, cx| state.set_value("", window, cx));
      self.store.update(cx, |store, cx| store.set_query(String::new(), cx));
      window.focus(&self.focus, cx);
    } else if self.store.read(cx).view != View::Home {
      let conv = self.store.read(cx).conv.clone();
      let closed = conv.is_some_and(|conv| {
        conv.update(cx, |conv, cx| {
          conv.close_find(window, cx) || conv.cancel_draft(window, cx) || conv.close_thread(window, cx)
        })
      });
      if !closed {
        self.go_home(window, cx);
      }
    }
  }

  /// A modal overlay has the keyboard: conversation hotkeys stay out of its way.
  const fn modal_open(&self) -> bool {
    self.settings.is_some() || self.switcher.is_some() || self.theme_picker.is_some() || self.new_chat.is_some()
  }

  /// Back and Forward through the views (Ctrl+Cmd+Left and Right, and the Go menu). A modal over the window keeps
  /// them out; the media viewer closes first, since it covers the view.
  fn step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
    if self.modal_open() {
      return;
    }
    if self.viewer.read(cx).is_open() {
      self.viewer.update(cx, |viewer, cx| viewer.close(window, cx));
      return;
    }
    self.store.update(cx, |store, cx| {
      if back {
        store.go_back(window, cx);
      } else {
        store.go_forward(window, cx);
      }
    });
    if self.store.read(cx).view == View::Home || matches!(self.store.read(cx).view, View::Shortcut(_)) {
      window.focus(&self.focus, cx);
    }
  }

  fn indicator(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let connection = self.store.read(cx).connection;
    let mine = cx.global::<status::Mine>().0.clone();
    let settings = cx.global::<settings::Settings>().clone();
    let (color, text) = status_menu::button(connection, &mine, settings.away, &settings);
    h_flex()
      .id("status-button")
      .gap_1p5()
      .px_2()
      .py_1()
      .items_center()
      .rounded(px(8.))
      .cursor_pointer()
      .hover(|style| style.bg(rgb(ROW_HOVER)))
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, event: &MouseDownEvent, window, cx| {
          cx.stop_propagation();
          if this.store.read(cx).connection != Connection::Online {
            return;
          }
          let core = this.store.read(cx).core.clone();
          status_menu::show(event.position, core, window, cx);
        }),
      )
      .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(rgb(color)))
      .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(text))
  }
}

impl Render for RootView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let store = self.store.read(cx);
    let signed_out = store.signed_out.clone();
    let at_home = store.view == View::Home;
    let on_shortcut = matches!(store.view, View::Shortcut(_));
    let open_conv = match (&store.view, &store.conv) {
      (View::Conversation(_), Some(conv)) => Some(conv.clone()),
      _ => None,
    };

    if signed_out.is_some() {
      if self.welcome.is_none() {
        let store = self.store.clone();
        let logo = self.logo.clone();
        self.welcome = Some(cx.new(|cx| WelcomeView::new(store, logo, cx)));
      }
      if let Some(welcome) = self.welcome.clone() {
        return welcome.into_any_element();
      }
    } else {
      self.welcome = None;
    }

    if (at_home || on_shortcut) && window.focused(cx).is_none() {
      let focus = self.focus.clone();
      window.defer(cx, move |window, cx| window.focus(&focus, cx));
    }

    let content: AnyElement = match open_conv {
      Some(conv) => conv.into_any_element(),
      None if on_shortcut => self.shortcuts.clone().into_any_element(),
      None => self.home.clone().into_any_element(),
    };

    // Own bar instead of a fixed-height title bar: tall enough for the search field, draggable, and with the
    // same 80px reserved on both sides so the field sits in the middle of the window.
    let bar = h_flex()
      .id("top-bar")
      .h(px(TOP_BAR_HEIGHT))
      .w_full()
      .flex_shrink_0()
      // Clear of the traffic lights, with a visible gap before the mark.
      .pl(px(92.))
      .pr_4()
      .gap_3()
      .items_center()
      .on_mouse_down(MouseButton::Left, |event, window, _| {
        if event.click_count == 2 {
          window.titlebar_double_click();
        } else {
          window.start_window_move();
        }
      })
      .child(
        h_flex()
          .w(px(160.))
          .flex_shrink_0()
          .gap_2p5()
          .items_center()
          .child(
            img(self.logo[usize::from(is_light())].clone())
              .size(px(24.))
              .flex_shrink_0(),
          )
          .child(
            div()
              .text_size(px(15.))
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(rgb(TEXT))
              .child("Working"),
          ),
      )
      .child(
        div().flex_1().min_w_0().flex().justify_center().child(
          h_flex()
            .w_full()
            .max_w(px(480.))
            .h(px(34.))
            .px_3()
            .gap_2()
            .items_center()
            .rounded_full()
            .bg(rgb(FIELD))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(icon(Icon::Search, 15., MUTED))
            .child(
              div()
                .flex_1()
                .child(Input::new(&self.search).appearance(false).bordered(false)),
            ),
        ),
      )
      .child(
        div()
          .w(px(252.))
          .flex_shrink_0()
          .flex()
          .justify_end()
          .child(self.indicator(cx)),
      );

    v_flex()
      .size_full()
      .bg(rgb(BG))
      .text_color(rgb(TEXT))
      .text_size(px(13.))
      .key_context("Root")
      .track_focus(&self.focus)
      .on_action(cx.listener(|this, _: &Escape, window, cx| this.on_escape(window, cx)))
      .on_action(cx.listener(|this, _: &OpenSwitcher, window, cx| this.open_switcher(window, cx)))
      .on_action(cx.listener(|this, _: &OpenThemePicker, window, cx| this.open_theme_picker(window, cx)))
      .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
      .on_action(cx.listener(|this, _: &OpenNotificationSettings, window, cx| this.open_settings_at(true, window, cx)))
      .on_action(cx.listener(|this, _: &GoHome, window, cx| this.go_home(window, cx)))
      .on_action(cx.listener(|this, _: &GoBack, window, cx| this.step(true, window, cx)))
      .on_action(cx.listener(|this, _: &GoForward, window, cx| this.step(false, window, cx)))
      .on_action(cx.listener(|this, _: &OpenNewChat, window, cx| this.open_new_chat(window, cx)))
      .on_action(cx.listener(|this, _: &RewriteDraft, window, cx| {
        if this.modal_open() {
          return;
        }
        let Some(conv) = this.store.read(cx).conv.clone() else {
          return;
        };
        conv.update(cx, |conv, cx| conv.rewrite_hotkey(window, cx));
      }))
      .on_action(cx.listener(|this, _: &FindInChat, window, cx| {
        if this.modal_open() {
          return;
        }
        if let Some(conv) = this.store.read(cx).conv.clone() {
          conv.update(cx, |conv, cx| conv.open_find(window, cx));
        } else {
          this.search.update(cx, |state, cx| state.focus(window, cx));
        }
      }))
      .on_action(cx.listener(|this, _: &FindNext, window, cx| {
        let Some(conv) = this.store.read(cx).conv.clone() else {
          return;
        };
        conv.update(cx, |conv, cx| conv.find_next(window, cx));
      }))
      .on_action(cx.listener(|this, _: &FindPrevious, window, cx| {
        let Some(conv) = this.store.read(cx).conv.clone() else {
          return;
        };
        conv.update(cx, |conv, cx| conv.find_prev(window, cx));
      }))
      .child(bar)
      .child(
        h_flex().flex_1().min_h_0().child(self.sidebar.clone()).child(
          div()
            .flex_1()
            .h_full()
            .min_w_0()
            .mr_2()
            .mb_2()
            .rounded(px(14.))
            .bg(rgb(PANEL))
            .overflow_hidden()
            .child(content),
        ),
      )
      .children(self.switcher.as_ref().map(|(switcher, _)| switcher.clone()))
      .children(self.theme_picker.as_ref().map(|(picker, _)| picker.clone()))
      .children(self.new_chat.as_ref().map(|(view, _)| view.clone()))
      .children(self.settings.as_ref().map(|(view, _)| view.clone()))
      .child(self.viewer.clone())
      .child(self.composer.clone())
      .child(self.menu.clone())
      .child(self.emoji.clone())
      .children(crate::updater::card(cx))
      .into_any_element()
  }
}
