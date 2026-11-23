//! Window root: title bar with search and status, the layout's columns, content panel, and overlays.

use chrono::NaiveDate;
use gchat_core::{Connection, Core, Shortcut};
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(target_os = "macos"))]
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
#[cfg(not(target_os = "macos"))]
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
#[cfg(not(target_os = "macos"))]
use std::cell::RefCell;
#[cfg(not(target_os = "macos"))]
use std::rc::Rc;

use crate::about::{About, AboutEvent};
use crate::calendar::{CalendarView, OpenCalendar};
use crate::choice_picker::{Choice, ChoicePicker, ChoicePickerEvent};
use crate::chrome;
use crate::conv_list::ConvList;
use crate::emoji_font;
use crate::emoji_picker::{EmojiHandle, EmojiPicker};
use crate::home::HomeView;
use crate::keys::{self, Hotkey1, Hotkey2, Hotkey3, Hotkey4, Hotkey5, Hotkey6, Hotkey7, Hotkey8, Hotkey9};
use crate::mail::{MailView, OpenMail};
use crate::meet::{MeetEvent, MeetView, OpenMeet};
use crate::menu::{ContextMenu, MenuHandle};
use crate::new_chat::{NewChat, NewChatEvent, OpenNewChat};
use crate::rail::{RAIL_WIDTH, Rail};
use crate::settings::{self, Layout, Place, SidebarMode, SidebarSide};
use crate::settings_view::{SettingsEvent, SettingsView};
use crate::shortcut::ShortcutView;
use crate::sidebar::Sidebar;
use crate::status;
use crate::status_menu::{self, ComposerHandle, OpenNotificationSettings, StatusComposer};
use crate::store::{Store, View};
use crate::switcher::{Switcher, SwitcherEvent};
use crate::text_panel::{TextPanel, TextPanelHandle};
use crate::theme::{self, ACCENT, BG, BORDER, FIELD, MUTED, PANEL, ROW_HOVER, TEXT, is_light, rgb};
use crate::theme_picker::{ThemePicker, ThemePickerEvent};
use crate::ui::{Icon, icon};
use crate::viewer::{Viewer, ViewerHandle};
use crate::welcome::WelcomeView;

actions!(
  working,
  [
    OpenSwitcher,
    OpenThemePicker,
    OpenLayoutPicker,
    OpenMessageStylePicker,
    OpenAbout,
    OpenSettings,
    GoHome,
    Quit,
    RewriteDraft,
    FindInChat,
    FindNext,
    FindPrevious,
    GoBack,
    GoForward,
    NextConversation,
    PreviousConversation,
    NextUnread,
    ToggleSidebar
  ]
);

// The app and window commands of the macOS menu bar. Elsewhere the title bar has the window's buttons, and hiding apps
// is not there (Windows' platform panics on it).
#[cfg(target_os = "macos")]
actions!(working, [Hide, HideOthers, ShowAll, Minimize, Zoom]);

/// Height of the top bar. On macOS the traffic lights are centered in it (see `chrome::set_title_bar`).
pub const TOP_BAR_HEIGHT: f32 = 52.0;

/// How long the floating sidebar takes to come in or go away, as gpui-kit's own sidebar does.
const OVERLAY_TRANSITION: Duration = Duration::from_millis(200);

/// How far the floating sidebar slides while it fades: a short move reads as quick; its full width crept in.
const OVERLAY_TRAVEL: f32 = 40.0;

/// How long the pointer may be away from the floating sidebar before it goes, so crossing its edge does not make it
/// flicker.
const OVERLAY_HIDE_DELAY: Duration = Duration::from_millis(150);

/// Gap between the floating sidebar and the window's left and bottom edges, as the panel keeps.
const OVERLAY_INSET: f32 = 8.0;

/// Width of the hot zone at the window's left edge that brings the hidden columns back as a floating overlay.
const HOT_ZONE_WIDTH: f32 = 12.0;

/// Width of the grab area on a column's inner edge.
const RESIZE_HANDLE_WIDTH: f32 = 8.0;

/// The gap between the content panel and the window's edge, or the column beside it (`mr_2`, `ml_2`).
const PANEL_MARGIN: f32 = 8.0;

/// How wide the column(s) of the layout are together.
fn columns_width(cx: &App) -> f32 {
  let settings = cx.global::<settings::Settings>();
  let rail = if settings.layout == Layout::Rail {
    RAIL_WIDTH
  } else {
    0.
  };
  settings.column_width(settings.layout) + rail
}

/// How wide the content panel is: the window less the column(s) beside it, while they are open, and its margins.
pub fn panel_width(window: &Window, cx: &App) -> f32 {
  let window_width = window.viewport_size().width.as_f32();
  if cx.global::<settings::Settings>().sidebar == SidebarMode::Open {
    window_width - columns_width(cx) - PANEL_MARGIN
  } else {
    2.0_f32.mul_add(-PANEL_MARGIN, window_width)
  }
}

/// The application menus: the macOS menu bar, and on Linux and Windows the submenus of the top bar's menu button. A
/// view with a number key (`Settings::hotkeys`) is listed with that key's action, so the menu shows the key.
pub fn menus(cx: &App) -> Vec<Menu> {
  let place = |name: &'static str, place: Place, action: Box<dyn Action>| MenuItem::Action {
    name: name.into(),
    action: cx
      .global::<settings::Settings>()
      .hotkey_of(&place)
      .and_then(keys::hotkey_action)
      .unwrap_or(action),
    os_action: None,
    checked: false,
    disabled: false,
  };
  let mut app = vec![
    MenuItem::action("About Working", OpenAbout),
    MenuItem::separator(),
    MenuItem::action("Settings...", OpenSettings),
    MenuItem::action("Color Theme…", OpenThemePicker),
    MenuItem::action("Layout…", OpenLayoutPicker),
    MenuItem::action("Message Style…", OpenMessageStylePicker),
    MenuItem::separator(),
  ];
  #[cfg(target_os = "macos")]
  app.extend([
    MenuItem::action("Hide Working", Hide),
    MenuItem::action("Hide Others", HideOthers),
    MenuItem::action("Show All", ShowAll),
    MenuItem::separator(),
  ]);
  app.push(MenuItem::action("Quit Working", Quit));
  let features = cx.global::<settings::Settings>().features();
  let mut go = vec![
    MenuItem::action("Back", GoBack),
    MenuItem::action("Forward", GoForward),
    MenuItem::separator(),
    place("Home", Place::Home, Box::new(GoHome)),
  ];
  // A feature that is off has no entry.
  if features.calendar {
    go.push(place("Calendar", Place::Calendar, Box::new(OpenCalendar)));
  }
  if features.meet {
    go.push(place("Meet", Place::Meet, Box::new(OpenMeet)));
  }
  if features.mail {
    go.push(place("Mail", Place::Mail, Box::new(OpenMail)));
  }
  go.extend([
    MenuItem::action("New Chat…", OpenNewChat),
    MenuItem::action("Quick Switcher…", OpenSwitcher),
    MenuItem::separator(),
    MenuItem::action("Next Conversation", NextConversation),
    MenuItem::action("Previous Conversation", PreviousConversation),
    MenuItem::action("Next Unread", NextUnread),
  ]);
  #[cfg_attr(
    not(target_os = "macos"),
    expect(unused_mut, reason = "only macOS adds the Window menu")
  )]
  let mut menus = vec![
    Menu {
      name: "Working".into(),
      disabled: false,
      items: app,
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
      name: "View".into(),
      disabled: false,
      items: vec![MenuItem::action("Toggle Sidebar", ToggleSidebar)],
    },
    Menu {
      name: "Go".into(),
      disabled: false,
      items: go,
    },
  ];
  #[cfg(target_os = "macos")]
  menus.push(Menu {
    name: "Window".into(),
    disabled: false,
    items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Zoom", Zoom)],
  });
  menus
}

/// A menu bar command: what it does to the main window's root view.
#[cfg(target_os = "macos")]
pub type MenuCommand = fn(&mut RootView, &mut Window, &mut Context<RootView>);

/// Answers the menu bar's commands for the app as a whole, each through `on_main`, which runs it on the main window.
/// The root view's own listeners answer while the focus is inside it; these answer when another window is in front or
/// nothing in the main window has the focus, so the menu items stay enabled.
#[cfg(target_os = "macos")]
pub fn bind_menu_commands(cx: &mut App, on_main: fn(&mut App, MenuCommand)) {
  fn bind<A: Action>(cx: &mut App, on_main: fn(&mut App, MenuCommand), run: MenuCommand) {
    cx.on_action(move |_: &A, cx| on_main(cx, run));
  }
  bind::<OpenSettings>(cx, on_main, RootView::open_settings);
  bind::<OpenThemePicker>(cx, on_main, RootView::open_theme_picker);
  bind::<OpenLayoutPicker>(cx, on_main, RootView::open_choice_picker::<Layout>);
  bind::<OpenMessageStylePicker>(cx, on_main, RootView::open_choice_picker::<settings::MessageStyle>);
  bind::<OpenAbout>(cx, on_main, RootView::open_about);
  bind::<FindInChat>(cx, on_main, RootView::find_in_chat);
  bind::<FindNext>(cx, on_main, RootView::find_next);
  bind::<FindPrevious>(cx, on_main, RootView::find_previous);
  bind::<RewriteDraft>(cx, on_main, RootView::rewrite_draft);
  bind::<ToggleSidebar>(cx, on_main, RootView::toggle_sidebar);
  bind::<GoBack>(cx, on_main, |root, window, cx| root.step(true, window, cx));
  bind::<GoForward>(cx, on_main, |root, window, cx| root.step(false, window, cx));
  bind::<GoHome>(cx, on_main, RootView::go_home);
  bind::<OpenCalendar>(cx, on_main, RootView::open_calendar);
  bind::<OpenMeet>(cx, on_main, RootView::open_meet);
  bind::<OpenMail>(cx, on_main, RootView::open_mail);
  bind::<OpenNewChat>(cx, on_main, RootView::open_new_chat);
  bind::<OpenSwitcher>(cx, on_main, RootView::open_switcher);
  bind::<NextConversation>(cx, on_main, |root, window, cx| root.step_conversation(true, window, cx));
  bind::<PreviousConversation>(cx, on_main, |root, window, cx| root.step_conversation(false, window, cx));
  bind::<NextUnread>(cx, on_main, RootView::open_next_unread);
  bind::<Hotkey1>(cx, on_main, |root, window, cx| root.open_hotkey(1, window, cx));
  bind::<Hotkey2>(cx, on_main, |root, window, cx| root.open_hotkey(2, window, cx));
  bind::<Hotkey3>(cx, on_main, |root, window, cx| root.open_hotkey(3, window, cx));
  bind::<Hotkey4>(cx, on_main, |root, window, cx| root.open_hotkey(4, window, cx));
  bind::<Hotkey5>(cx, on_main, |root, window, cx| root.open_hotkey(5, window, cx));
  bind::<Hotkey6>(cx, on_main, |root, window, cx| root.open_hotkey(6, window, cx));
  bind::<Hotkey7>(cx, on_main, |root, window, cx| root.open_hotkey(7, window, cx));
  bind::<Hotkey8>(cx, on_main, |root, window, cx| root.open_hotkey(8, window, cx));
  bind::<Hotkey9>(cx, on_main, |root, window, cx| root.open_hotkey(9, window, cx));
}

/// Off macOS: `items` added to `popup`, a submenu as a submenu beside it. The system menus are macOS's alone.
#[cfg(not(target_os = "macos"))]
fn fill_menu(
  mut popup: PopupMenu,
  items: &[OwnedMenuItem],
  window: &mut Window,
  cx: &mut Context<PopupMenu>,
) -> PopupMenu {
  for item in items {
    popup = match item {
      OwnedMenuItem::Action { name, action, checked, disabled, .. } => popup.item(
        PopupMenuItem::new(name.clone())
          .action(action.boxed_clone())
          .checked(*checked)
          .disabled(*disabled),
      ),
      OwnedMenuItem::Separator => popup.separator(),
      OwnedMenuItem::Submenu(menu) => {
        let items = menu.items.clone();
        popup.submenu(menu.name.clone(), window, cx, move |popup, window, cx| {
          fill_menu(popup, &items, window, cx)
        })
      },
      OwnedMenuItem::SystemMenu(_) => popup,
    };
  }
  popup
}

/// The layout or message style picker on screen.
struct OpenChoicePicker {
  view: AnyView,
  /// Puts the saved choice back on screen.
  restore: Box<dyn Fn(&mut App)>,
  _sub: Subscription,
}

pub struct RootView {
  store: Entity<Store>,
  sidebar: Entity<Sidebar>,
  conv_list: Entity<ConvList>,
  rail: Entity<Rail>,
  home: Entity<HomeView>,
  shortcuts: Entity<ShortcutView>,
  calendar: Entity<CalendarView>,
  meet: Entity<MeetView>,
  mail: Entity<MailView>,
  search: Entity<InputState>,
  focus: FocusHandle,
  /// The top-bar mark, for the dark look and for the light one.
  logo: [Arc<Image>; 2],
  /// Off macOS, where the system has no menu bar: what had focus when the menu button was pressed, so the menu's
  /// Edit commands reach that field.
  #[cfg(not(target_os = "macos"))]
  menu_target: Rc<RefCell<Option<FocusHandle>>>,
  viewer: Entity<Viewer>,
  menu: Entity<ContextMenu>,
  text_panel: Entity<TextPanel>,
  emoji: Entity<EmojiPicker>,
  composer: Entity<StatusComposer>,
  switcher: Option<(Entity<Switcher>, Subscription)>,
  theme_picker: Option<(Entity<ThemePicker>, Subscription)>,
  choice_picker: Option<OpenChoicePicker>,
  about: Option<(Entity<About>, Subscription)>,
  settings: Option<(Entity<SettingsView>, Subscription)>,
  new_chat: Option<(Entity<NewChat>, Subscription)>,
  welcome: Option<Entity<WelcomeView>>,
  /// The layout on screen, to tell when the setting changes.
  layout: Layout,
  /// Auto-hide floats the column(s) over the panel, opened from this view: going to another view closes them.
  overlay: Option<View>,
  /// The overlay is playing its way out; the pointer coming back turns it around.
  overlay_leaving: bool,
  /// A column edge is being dragged: where the pointer went down, and the column's width then.
  resizing: Option<(Pixels, f32)>,
  /// Slides the overlay away once the pointer has been out of it for a moment.
  _overlay_hide: Option<Task<()>>,
  _subs: Vec<Subscription>,
  _pump: Task<()>,
  _opened: Task<()>,
}

impl RootView {
  pub fn new(core: Core, store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let sidebar = cx.new(|cx| Sidebar::new(store.clone(), cx));
    let conv_list = cx.new(|cx| ConvList::new(store.clone(), cx));
    let rail = cx.new(|cx| Rail::new(store.clone(), cx));
    let layout = cx.global::<settings::Settings>().layout;
    store.update(cx, super::store::Store::sync_settings);
    let home = cx.new(|cx| HomeView::new(store.clone(), cx));
    let shortcuts = cx.new(|_| ShortcutView::new(store.clone()));
    let calendar = cx.new(|cx| CalendarView::new(store.clone(), window, cx));
    let meet = cx.new(|cx| MeetView::new(store.clone(), window, cx));
    let mail = cx.new(|cx| MailView::new(store.clone(), window, cx));
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search chat"));
    let focus = cx.focus_handle();
    window.focus(&focus, cx);
    let images = store.read(cx).images.clone();
    let viewer = cx.new(|cx| Viewer::new(core.clone(), images, cx));
    let menu = cx.new(ContextMenu::new);
    let text_panel = cx.new(|cx| TextPanel::new(store.clone(), window, cx));
    let emoji = cx.new(|cx| EmojiPicker::new(window, cx));
    let composer = cx.new(|cx| StatusComposer::new(core.clone(), window, cx));
    cx.set_global(ViewerHandle(viewer.clone()));
    cx.set_global(MenuHandle(menu.clone()));
    cx.set_global(TextPanelHandle(text_panel.clone()));
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
        // Going to another view ends a pick in the floating sidebar.
        this.sync_overlay(cx);
        // The panel shows the store's view: a key that opens a conversation repaints nothing else.
        cx.notify();
      }),
      // "System" appearance follows the system's as it changes.
      cx.observe_window_appearance(window, |_, _, cx| settings::apply_appearance(cx)),
      // The layout rearranges the window; the store takes it and fits its pane to it (`Store::sync_settings`). The sort
      // orders and the auto-hide switch follow the settings too.
      cx.observe_global::<settings::Settings>(|this, cx| {
        let layout = cx.global::<settings::Settings>().layout;
        if layout != this.layout {
          this.layout = layout;
          this.close_overlay_state();
        }
        this.store.update(cx, super::store::Store::sync_settings);
        this.sync_overlay(cx);
        cx.notify();
      }),
      cx.observe_global::<status::Mine>(|_, cx| cx.notify()),
      // The window's own overlays coming and going decide whether Mail's webviews, native views drawn over GPUI's,
      // may show (`covers_panel`).
      cx.observe(&menu, |_, _, cx| cx.notify()),
      cx.observe(&viewer, |_, _, cx| cx.notify()),
      cx.observe(&text_panel, |_, _, cx| cx.notify()),
      cx.observe(&emoji, |_, _, cx| cx.notify()),
      cx.observe(&composer, |_, _, cx| cx.notify()),
      // Mail's labels popover is one of them.
      cx.observe(&mail, |_, _, cx| cx.notify()),
      // Meet's "Schedule in Google Calendar" opens the calendar's editor.
      cx.subscribe_in(&meet, window, |this, _, event: &MeetEvent, window, cx| {
        let MeetEvent::Schedule(day) = event;
        this.schedule_meeting(*day, window, cx);
      }),
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

    // A clicked notification opens its conversation, or the chat of its call.
    let clicks = crate::notify::start();
    let target = store.clone();
    let opened = cx.spawn_in(window, async move |_, cx| {
      while let Ok(key) = clicks.recv_async().await {
        let target = target.clone();
        let opening = cx.update(|window, cx| {
          if crate::call::show_chat(&key, cx) {
            return;
          }
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
      conv_list,
      rail,
      home,
      search,
      focus,
      shortcuts,
      calendar,
      meet,
      mail,
      #[cfg(not(target_os = "macos"))]
      menu_target: Rc::default(),
      logo,
      viewer,
      menu,
      text_panel,
      emoji,
      composer,
      switcher: None,
      theme_picker: None,
      choice_picker: None,
      about: None,
      settings: None,
      new_chat: None,
      welcome: None,
      layout,
      overlay: None,
      overlay_leaving: false,
      resizing: None,
      _overlay_hide: None,
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
    let sub = cx.subscribe_in(&view, window, |this, _, _: &SettingsEvent, window, cx| {
      this.settings = None;
      this.refocus(window, cx);
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

  /// Opens the picker of a setting's choices: the layout, or the message style. One at a time.
  fn open_choice_picker<T: Choice>(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.choice_picker.is_some() {
      return;
    }
    let picker = cx.new(|cx| ChoicePicker::<T>::new(window, cx));
    let saved = picker.read(cx).saved;
    let sub = cx.subscribe_in(&picker, window, |this, _, event: &ChoicePickerEvent<T>, window, cx| {
      let picked = match *event {
        ChoicePickerEvent::Pick(value) => {
          settings::Settings::preview(cx, |settings| value.set(settings));
          true
        },
        ChoicePickerEvent::Close => false,
      };
      this.close_choice_picker(picked, window, cx);
    });
    self.choice_picker = Some(OpenChoicePicker {
      view: picker.into(),
      restore: Box::new(move |cx| {
        settings::Settings::preview(cx, |settings| saved.set(settings));
      }),
      _sub: sub,
    });
    cx.notify();
  }

  /// Closes the picker: `picked` saves the choice on screen, and without a pick the saved one comes back.
  fn close_choice_picker(&mut self, picked: bool, window: &mut Window, cx: &mut Context<Self>) {
    let Some(picker) = self.choice_picker.take() else {
      return;
    };
    if picked {
      settings::Settings::commit(cx);
    } else {
      (picker.restore)(cx);
    }
    self.refocus(window, cx);
    cx.notify();
  }

  fn open_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.about.is_some() {
      return;
    }
    let logo = self.logo.clone();
    let about = cx.new(|cx| About::new(logo, window, cx));
    let sub = cx.subscribe_in(&about, window, |this, _, _: &AboutEvent, window, cx| {
      this.close_about(window, cx)
    });
    self.about = Some((about, sub));
    cx.notify();
  }

  fn close_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.about.take().is_none() {
      return;
    }
    self.refocus(window, cx);
    cx.notify();
  }

  /// Number key `slot`: opens the place the settings give it, if any.
  fn open_hotkey(&mut self, slot: u8, window: &mut Window, cx: &mut Context<Self>) {
    let settings = cx.global::<settings::Settings>();
    let Some(place) = settings.hotkeys.get(&slot).cloned() else {
      return;
    };
    // A feature that is off keeps its key, and the key does nothing.
    if !settings.features().opens(&place) {
      return;
    }
    match place {
      Place::Home => self.go_home(window, cx),
      Place::Calendar => self.open_calendar(window, cx),
      Place::Meet => self.open_meet(window, cx),
      Place::Mail => self.open_mail(window, cx),
      Place::Mentions => self.open_shortcut(Shortcut::Mentions, window, cx),
      Place::Starred => self.open_shortcut(Shortcut::Starred, window, cx),
      Place::Conversation(id) => {
        // A conversation that was left keeps its key, and the key does nothing.
        if self.store.read(cx).group(&id).is_none() || !self.can_navigate(window, cx) {
          return;
        }
        self.close_overlay_state();
        self.store.update(cx, |store, cx| store.open(id, window, cx));
      },
    }
  }

  fn open_shortcut(&mut self, shortcut: Shortcut, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.close_overlay_state();
    self.store.update(cx, |store, cx| store.open_shortcut(shortcut, cx));
    window.focus(&self.focus, cx);
  }

  fn rewrite_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.modal_open(cx) {
      return;
    }
    if self.store.read(cx).view == crate::store::View::Mail {
      self.mail.update(cx, |mail, cx| mail.rewrite_hotkey(window, cx));
      return;
    }
    let Some(conv) = self.store.read(cx).conv.clone() else {
      return;
    };
    conv.update(cx, |conv, cx| conv.rewrite_hotkey(window, cx));
  }

  fn find_in_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.modal_open(cx) {
      return;
    }
    if let Some(conv) = self.store.read(cx).conv.clone() {
      conv.update(cx, |conv, cx| conv.open_find(window, cx));
    } else {
      self.search.update(cx, |state, cx| state.focus(window, cx));
    }
  }

  fn find_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(conv) = self.store.read(cx).conv.clone() else {
      return;
    };
    conv.update(cx, |conv, cx| conv.find_next(window, cx));
  }

  fn find_previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(conv) = self.store.read(cx).conv.clone() else {
      return;
    };
    conv.update(cx, |conv, cx| conv.find_prev(window, cx));
  }

  /// Closes the picker and shows the saved theme again, which is the picked one after a pick.
  fn close_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.theme_picker.take().is_none() {
      return;
    }
    settings::apply_appearance(cx);
    self.refocus(window, cx);
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
    self.refocus(window, cx);
    cx.notify();
  }

  fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.switcher = None;
    self.refocus(window, cx);
    cx.notify();
  }

  /// Gives the keyboard back once an overlay closes: to the open conversation's composer, to the calendar, Meet, or
  /// Mail, or to the window.
  fn refocus(&self, window: &mut Window, cx: &mut Context<Self>) {
    if self.store.read(cx).view == View::Calendar {
      self.focus_calendar(window, cx);
      return;
    }
    if self.store.read(cx).view == View::Meet {
      self.focus_meet(window, cx);
      return;
    }
    if self.store.read(cx).view == View::Mail {
      self.focus_mail(window, cx);
      return;
    }
    match self.store.read(cx).conv.clone() {
      Some(conv) => conv.update(cx, |conv, cx| conv.focus_composer(window, cx)),
      None => window.focus(&self.focus, cx),
    }
  }

  /// Shows Google Calendar and gives it the keyboard, for its one-letter keys.
  fn open_calendar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.switcher = None;
    self.new_chat = None;
    self.close_overlay_state();
    self.store.update(cx, super::store::Store::open_calendar);
    // Calendar stays shut while it is off.
    if self.store.read(cx).view == View::Calendar {
      self.focus_calendar(window, cx);
    }
  }

  fn focus_calendar(&self, window: &mut Window, cx: &mut Context<Self>) {
    let focus = self.calendar.read(cx).focus_handle();
    window.focus(&focus, cx);
  }

  /// Meet's "Schedule in Google Calendar": shows the calendar on `day`, with the editor open on a new event that has
  /// Google Meet.
  fn schedule_meeting(&mut self, day: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
    self.open_calendar(window, cx);
    // A modal over the window refuses the navigation: the calendar is not on screen then.
    if self.store.read(cx).view == View::Calendar {
      self.calendar.update(cx, |calendar, cx| calendar.new_meeting(day, window, cx));
    }
  }

  /// Shows Meet and gives it the keyboard, for its `/` key.
  fn open_meet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.switcher = None;
    self.new_chat = None;
    self.close_overlay_state();
    self.store.update(cx, super::store::Store::open_meet);
    // Meet stays shut while it is off.
    if self.store.read(cx).view == View::Meet {
      self.focus_meet(window, cx);
    }
  }

  fn focus_meet(&self, window: &mut Window, cx: &mut Context<Self>) {
    let focus = self.meet.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
  }

  /// Shows Gmail and gives it the keyboard.
  fn open_mail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.switcher = None;
    self.new_chat = None;
    self.close_overlay_state();
    self.store.update(cx, super::store::Store::open_mail);
    // Mail stays shut while it is off: Gmail is not read then.
    if self.store.read(cx).view == View::Mail {
      self.focus_mail(window, cx);
    }
  }

  fn focus_mail(&self, window: &mut Window, cx: &mut Context<Self>) {
    let focus = self.mail.read(cx).focus_handle();
    window.focus(&focus, cx);
  }

  fn sidebar_mode(cx: &App) -> SidebarMode {
    cx.global::<settings::Settings>().sidebar
  }

  fn sidebar_side(cx: &App) -> SidebarSide {
    cx.global::<settings::Settings>().sidebar_side
  }

  /// Drags the edge of the layout's resizable column, following the pointer; saves once the button is up.
  fn drag_column(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
    let Some((start, width)) = self.resizing else {
      return;
    };
    if event.pressed_button != Some(MouseButton::Left) {
      self.end_resize(cx);
      return;
    }
    let moved = (event.position.x - start).as_f32();
    let moved = match Self::sidebar_side(cx) {
      SidebarSide::Left => moved,
      SidebarSide::Right => -moved,
    };
    let layout = self.layout;
    let width = (width + moved).clamp(settings::COLUMN_MIN_WIDTH, settings::COLUMN_MAX_WIDTH);
    settings::Settings::preview(cx, |settings| {
      settings.column_widths.insert(layout, width);
    });
  }

  fn end_resize(&mut self, cx: &mut Context<Self>) {
    if self.resizing.take().is_some() {
      settings::Settings::commit(cx);
      cx.notify();
    }
  }

  /// The grip on the inner edge of the open column(s): drag to resize, double-click for the default width.
  fn resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
    let edge = px(columns_width(cx) - RESIZE_HANDLE_WIDTH / 2.);
    let dragging = self.resizing.is_some();
    div()
      .id("column-resize")
      .absolute()
      .top_0()
      .bottom_0()
      .map(|handle| match Self::sidebar_side(cx) {
        SidebarSide::Left => handle.left(edge),
        SidebarSide::Right => handle.right(edge),
      })
      .w(px(RESIZE_HANDLE_WIDTH))
      .flex()
      .justify_center()
      .cursor(CursorStyle::ResizeLeftRight)
      .child(
        div()
          .w(px(2.))
          .h_full()
          .when(dragging, |line| line.bg(rgb(ACCENT)))
          .when(!dragging, |line| line.hover(|style| style.bg(rgb(BORDER)))),
      )
      .on_mouse_down(
        MouseButton::Left,
        cx.listener(|this, event: &MouseDownEvent, _, cx| {
          cx.stop_propagation();
          if event.click_count == 2 {
            let layout = this.layout;
            settings::Settings::update(cx, |settings| {
              settings.column_widths.remove(&layout);
            });
            return;
          }
          let width = cx.global::<settings::Settings>().column_width(this.layout);
          this.resizing = Some((event.position.x, width));
          cx.notify();
        }),
      )
      .into_any_element()
  }

  /// Forgets the floating sidebar, without touching the keyboard.
  fn close_overlay_state(&mut self) {
    self.overlay = None;
    self.overlay_leaving = false;
    self._overlay_hide = None;
  }

  /// Closes the floating sidebar once it has no reason to be: auto-hide is off, or a pick in it took the window to
  /// another view.
  fn sync_overlay(&mut self, cx: &App) {
    let Some(opened_from) = &self.overlay else {
      return;
    };
    if Self::sidebar_mode(cx) != SidebarMode::AutoHide || *opened_from != self.store.read(cx).view {
      self.close_overlay_state();
    }
  }

  /// Auto-hide: floats the columns in over the panel. Nothing moves the keyboard: typing in a composer goes on.
  fn open_overlay(&mut self, cx: &mut Context<Self>) {
    if self.overlay.is_some() || Self::sidebar_mode(cx) != SidebarMode::AutoHide {
      return;
    }
    self.overlay = Some(self.store.read(cx).view.clone());
    self.overlay_leaving = false;
    self._overlay_hide = None;
    cx.notify();
  }

  /// Slides the floating sidebar away, and gives the keyboard back.
  fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.overlay.is_none() {
      return;
    }
    self.close_overlay_state();
    self.refocus(window, cx);
    cx.notify();
  }

  /// Slides the floating sidebar away after a moment, unless the pointer comes back first. A context menu opened
  /// from it keeps it up until the menu closes.
  fn schedule_overlay_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.overlay.is_none() {
      return;
    }
    self._overlay_hide = Some(cx.spawn_in(window, async move |this, cx| {
      cx.background_executor().timer(OVERLAY_HIDE_DELAY).await;
      // A context menu opened from it keeps it up until the menu closes.
      loop {
        let waiting = this.update(cx, |this, cx| this.menu.read(cx).is_open()).unwrap_or(false);
        if !waiting {
          break;
        }
        cx.background_executor().timer(OVERLAY_HIDE_DELAY).await;
      }
      let leaving = this.update(cx, |this, cx| {
        this.overlay_leaving = this.overlay.is_some();
        cx.notify();
        this.overlay_leaving
      });
      if !leaving.unwrap_or(false) {
        return;
      }
      cx.background_executor().timer(OVERLAY_TRANSITION).await;
      this
        .update_in(cx, |this, window, cx| {
          if !this.overlay_leaving {
            return;
          }
          this.close_overlay_state();
          // Keeps a click made meanwhile elsewhere, such as the search field.
          if window.focused(cx).is_none_or(|focused| focused == this.focus) {
            this.refocus(window, cx);
          }
          cx.notify();
        })
        .ok();
    }));
  }

  /// Auto-hide follows the pointer, window-wide: at the sidebar's edge of the window, below the top bar, it floats
  /// the columns in; while they are in, leaving them hides them after a moment, and coming back keeps them.
  fn track_pointer(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
    if Self::sidebar_mode(cx) != SidebarMode::AutoHide {
      return;
    }
    // Distance from the sidebar's edge of the window.
    let from_edge = match Self::sidebar_side(cx) {
      SidebarSide::Left => position.x,
      SidebarSide::Right => window.viewport_size().width - position.x,
    };
    if self.overlay.is_none() {
      if from_edge < px(HOT_ZONE_WIDTH) && position.y > px(TOP_BAR_HEIGHT) {
        self.open_overlay(cx);
      }
      return;
    }
    if from_edge < px(columns_width(cx) + OVERLAY_INSET) {
      self._overlay_hide = None;
      if self.overlay_leaving {
        self.overlay_leaving = false;
        cx.notify();
      }
    } else if self._overlay_hide.is_none() {
      self.schedule_overlay_hide(window, cx);
    }
  }

  fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.switcher = None;
    self.new_chat = None;
    self.close_overlay_state();
    self.store.update(cx, super::store::Store::go_home);
    window.focus(&self.focus, cx);
  }

  fn on_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.text_panel.read(cx).is_open() {
      self.text_panel.update(cx, |panel, cx| panel.close(window, cx));
      return;
    }
    if self.composer.read(cx).is_open() && !self.emoji.read(cx).is_open() {
      self.composer.update(cx, |composer, cx| composer.close(window, cx));
    } else if self.emoji.read(cx).is_open() {
      self.emoji.update(cx, |emoji, cx| emoji.close(window, cx));
    } else if self.settings.take().is_some() {
      cx.notify();
    } else if self.menu.read(cx).is_open() {
      self.menu.update(cx, |menu, cx| menu.close(window, cx));
    } else if self.calendar.update(cx, |calendar, cx| calendar.dismiss(window, cx)) {
      // The event editor or card closed.
    } else if self.viewer.read(cx).is_open() {
      self.viewer.update(cx, |viewer, cx| viewer.close(window, cx));
    } else if self.new_chat.is_some() {
      self.close_new_chat(window, cx);
    } else if self.switcher.is_some() {
      self.close_switcher(window, cx);
    } else if self.theme_picker.is_some() {
      self.close_theme_picker(window, cx);
    } else if self.choice_picker.is_some() {
      self.close_choice_picker(false, window, cx);
    } else if self.about.is_some() {
      self.close_about(window, cx);
    } else if self.overlay.is_some() {
      self.close_overlay(window, cx);
    } else if self.meet.update(cx, |meet, cx| meet.dismiss(window, cx)) {
      // The link card closed, or a text field of Meet gave the keyboard back.
    } else if self.mail.update(cx, |mail, cx| mail.dismiss(window, cx)) {
      // The search field of Mail gave the keyboard back, or the thread open closed.
    } else if !self.store.read(cx).query.is_empty() {
      self.search.update(cx, |state, cx| state.set_value("", window, cx));
      self.store.update(cx, |store, cx| store.set_query(String::new(), cx));
      window.focus(&self.focus, cx);
    } else if self.store.read(cx).view != View::Home {
      let conv = self.store.read(cx).conv.clone();
      let closed = conv.is_some_and(|conv| {
        // A thread pop-up closes first: it covers the find bar and the main composer's draft.
        conv.update(cx, |conv, cx| {
          conv.close_thread_popup(window, cx)
            || conv.close_find(window, cx)
            || conv.cancel_draft(window, cx)
            || conv.close_thread(window, cx)
        })
      });
      // Messages and Rail keep their list beside the conversation: there is no Home to go back to.
      if !closed && !self.layout.keeps_list() {
        self.go_home(window, cx);
      }
    }
  }

  /// A modal overlay has the keyboard: conversation hotkeys stay out of its way.
  fn modal_open(&self, cx: &App) -> bool {
    self.settings.is_some()
      || self.text_panel.read(cx).is_open()
      || self.switcher.is_some()
      || self.theme_picker.is_some()
      || self.choice_picker.is_some()
      || self.about.is_some()
      || self.new_chat.is_some()
      || self.calendar.read(cx).has_modal()
  }

  /// Something the window draws over the content panel is open: a modal, a menu, a picker, the media viewer, the
  /// status composer, or the floating sidebar. Mail's webviews hide meanwhile (`MailView::set_covered`).
  fn covers_panel(&self, cx: &App) -> bool {
    self.modal_open(cx)
      || self.overlay.is_some()
      || self.menu.read(cx).is_open()
      || self.viewer.read(cx).is_open()
      || self.emoji.read(cx).is_open()
      || self.composer.read(cx).is_open()
      || self.mail.read(cx).labels_open
  }

  /// The open conversation's thread as a pop-up over the window, when one is open and the setting asks for that.
  fn thread_popup(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
    let store = self.store.read(cx);
    if !matches!(store.view, View::Conversation(_)) {
      return None;
    }
    let conv = store.conv.clone()?;
    let viewport = window.viewport_size();
    conv.update(cx, |conv, cx| conv.thread_popup(viewport, cx))
  }

  /// Whether a navigation hotkey goes through: a modal over the window keeps it out, and the media viewer closes
  /// first, since it covers the view.
  fn can_navigate(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
    if self.modal_open(cx) {
      return false;
    }
    if self.viewer.read(cx).is_open() {
      self.viewer.update(cx, |viewer, cx| viewer.close(window, cx));
      return false;
    }
    true
  }

  /// Back and Forward through the views (Cmd+[ and Cmd+] on macOS, Ctrl elsewhere, and the Go menu).
  fn step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.store.update(cx, |store, cx| {
      if back {
        store.go_back(window, cx);
      } else {
        store.go_forward(window, cx);
      }
    });
    match self.store.read(cx).view {
      View::Home | View::Shortcut(_) => window.focus(&self.focus, cx),
      View::Calendar => self.focus_calendar(window, cx),
      View::Meet => self.focus_meet(window, cx),
      View::Mail => self.focus_mail(window, cx),
      View::Conversation(_) => {},
    }
  }

  /// Opens the next or previous conversation in the order the layout lists them.
  fn step_conversation(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.store.update(cx, |store, cx| store.step_conversation(forward, window, cx));
  }

  /// Opens the newest unread conversation other than the open one.
  fn open_next_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    self.store.update(cx, |store, cx| store.open_next_unread(window, cx));
  }

  /// The sidebar button's click and its key (`keys::TOGGLE_SIDEBAR`): open closes, closed or auto-hide opens.
  fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if !self.can_navigate(window, cx) {
      return;
    }
    let next = match Self::sidebar_mode(cx) {
      SidebarMode::Open => SidebarMode::Closed,
      SidebarMode::Closed | SidebarMode::AutoHide => SidebarMode::Open,
    };
    settings::Settings::update(cx, |settings| settings.sidebar = next);
  }

  /// The top bar's sidebar button: click opens or closes the left column(s), right-click sets auto-hide (again:
  /// opens). Auto-hide shows as a dashed icon in the accent color, as the composer's rewrite button marks
  /// rewrite-always.
  fn sidebar_button(cx: &mut Context<Self>) -> impl IntoElement {
    let mode = Self::sidebar_mode(cx);
    let tip = match mode {
      SidebarMode::Open => "Close the sidebar. Right-click to auto-hide it",
      SidebarMode::Closed => "Open the sidebar. Right-click to auto-hide it",
      SidebarMode::AutoHide => "Auto-hiding: point at the window's edge. Click to keep the sidebar open",
    };
    div()
      .id("sidebar-button")
      .occlude()
      .size(px(30.))
      .flex_shrink_0()
      .flex()
      .items_center()
      .justify_center()
      .rounded_full()
      .cursor_pointer()
      .hover(|style| style.bg(rgb(ROW_HOVER)))
      .tooltip(move |window, cx| Tooltip::new(tip).action(&ToggleSidebar, None).build(window, cx))
      // The bar under it moves the window on a press.
      .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
      .on_mouse_down(MouseButton::Right, |_, _, cx| {
        cx.stop_propagation();
        settings::Settings::update(cx, |settings| {
          settings.sidebar = if settings.sidebar == SidebarMode::AutoHide {
            SidebarMode::Open
          } else {
            SidebarMode::AutoHide
          };
        });
      })
      .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)))
      .child(match (mode, Self::sidebar_side(cx)) {
        (SidebarMode::AutoHide, SidebarSide::Left) => icon(Icon::PanelLeftDashed, 17., ACCENT),
        (SidebarMode::AutoHide, SidebarSide::Right) => icon(Icon::PanelRightDashed, 17., ACCENT),
        (SidebarMode::Open | SidebarMode::Closed, SidebarSide::Left) => icon(Icon::PanelLeft, 17., TEXT),
        (SidebarMode::Open | SidebarMode::Closed, SidebarSide::Right) => icon(Icon::PanelRight, 17., TEXT),
      })
  }

  /// The column(s) of the layout, left to right: the sidebar, the list (Messages' or IRC's buffer list), or the rail
  /// and the list, with the rail at the window's edge on either side. Each is a cached view laid out as its own root
  /// lays itself out, which redraws itself when what it draws changes.
  fn columns(&self, cx: &App) -> Vec<AnyElement> {
    let column = |width: f32| StyleRefinement::default().w(px(width)).h_full().flex_shrink_0();
    let settings = cx.global::<settings::Settings>();
    match self.layout {
      Layout::GoogleChat => {
        let width = settings.column_width(Layout::GoogleChat);
        vec![self.sidebar.clone().cached(column(width)).into_any_element()]
      },
      Layout::Messages | Layout::Irc => {
        let width = settings.column_width(self.layout);
        vec![self.conv_list.clone().cached(column(width)).into_any_element()]
      },
      Layout::Rail => {
        let list = settings.column_width(Layout::Rail);
        let mut columns = vec![
          self.rail.clone().cached(column(RAIL_WIDTH)).into_any_element(),
          self.conv_list.clone().cached(column(list)).into_any_element(),
        ];
        if Self::sidebar_side(cx) == SidebarSide::Right {
          columns.reverse();
        }
        columns
      },
    }
  }

  /// The hidden column(s) as a floating card over the panel: inset from the window's edges, rounded like the panel,
  /// same widths, no scrim, nothing reflows. It leaves when the pointer does.
  fn overlay_element(&self, cx: &App) -> AnyElement {
    let width = columns_width(cx);
    let right = Self::sidebar_side(cx) == SidebarSide::Right;
    let card = h_flex()
      .id("sidebar-overlay")
      .absolute()
      .top_0()
      .bottom(px(OVERLAY_INSET))
      .w(px(width))
      .rounded(px(14.))
      .bg(rgb(BG))
      .border_1()
      .border_color(rgb(BORDER))
      .shadow_lg()
      .overflow_hidden()
      .occlude()
      .children(self.columns(cx));
    // In: slide away from the window's edge and fade in. Out: back toward it and fade out. Each direction is its own animation, so turning
    // around restarts from the other end.
    let leaving = self.overlay_leaving;
    let id = if leaving { "overlay-out" } else { "overlay-in" };
    card
      .with_animation(
        id,
        Animation::new(OVERLAY_TRANSITION).with_easing(ease_in_out),
        move |panel, delta| {
          let shown = if leaving { 1. - delta } else { delta };
          let offset = px(OVERLAY_TRAVEL.mul_add(shown - 1., OVERLAY_INSET));
          let panel = if right {
            panel.right(offset)
          } else {
            panel.left(offset)
          };
          panel.opacity(shown)
        },
      )
      .into_any_element()
  }

  fn indicator(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let connection = self.store.read(cx).connection;
    let mine = cx.global::<status::Mine>().0.clone();
    let settings = cx.global::<settings::Settings>().clone();
    let (color, text) = status_menu::button(connection, &mine, settings.away, &settings);
    h_flex()
      .id("status-button")
      .occlude()
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

  /// The app's menus off macOS, where the system has no menu bar: a button whose dropdown has each of [`menus`] as a
  /// submenu, with the shortcuts as this OS writes them.
  #[cfg(not(target_os = "macos"))]
  fn menu_button(&self, cx: &App) -> impl IntoElement {
    let pressed = self.menu_target.clone();
    let target = self.menu_target.clone();
    let hover = rgb(ROW_HOVER).into();
    div()
      .flex_shrink_0()
      .occlude()
      // The menu takes focus as it opens: remember the field that had it first.
      .capture_any_mouse_down(move |_, window, cx| *pressed.borrow_mut() = window.focused(cx))
      .child(
        Button::new("app-menu")
          .custom(ButtonCustomVariant::new(cx).hover(hover).active(hover))
          .size(px(30.))
          .rounded(px(15.))
          .tooltip("Menu")
          .child(icon(Icon::More, 17., TEXT))
          .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, window, cx| {
            let popup = match target.borrow().clone() {
              Some(focus) => popup.action_context(focus),
              None => popup,
            };
            let menus: Vec<OwnedMenuItem> =
              menus(cx).into_iter().map(|menu| OwnedMenuItem::Submenu(menu.owned())).collect();
            fill_menu(popup, &menus, window, cx)
          }),
      )
  }
}

impl Render for RootView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let store = self.store.read(cx);
    let signed_out = store.signed_out.clone();
    let at_home = store.view == View::Home;
    let on_shortcut = matches!(store.view, View::Shortcut(_));
    let on_calendar = store.view == View::Calendar;
    let on_meet = store.view == View::Meet;
    let on_mail = store.view == View::Mail;
    let covered = self.covers_panel(cx);
    // Only a change of cover writes to the Mail view: an update of another entity on every root render is a write
    // made while the window draws.
    if self.mail.read(cx).covered() != covered {
      self.mail.update(cx, |mail, cx| mail.set_covered(covered, cx));
    }
    let store = self.store.read(cx);
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
    if on_calendar && window.focused(cx).is_none() {
      let calendar = self.calendar.clone();
      window.defer(cx, move |window, cx| {
        let focus = calendar.read(cx).focus_handle();
        window.focus(&focus, cx);
      });
    }
    if on_meet && window.focused(cx).is_none() {
      let meet = self.meet.clone();
      window.defer(cx, move |window, cx| {
        let focus = meet.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
      });
    }
    if on_mail && window.focused(cx).is_none() {
      let mail = self.mail.clone();
      window.defer(cx, move |window, cx| {
        let focus = mail.read(cx).focus_handle();
        window.focus(&focus, cx);
      });
    }

    // Home and the conversation redraw themselves when what they draw changes, so they are cached. The others redraw
    // with the window root: Mentions and Starred, Calendar, Meet, and Mail each read more than they observe.
    let content: AnyElement = match open_conv {
      Some(conv) => conv.cached(StyleRefinement::default().size_full()).into_any_element(),
      None if on_shortcut => self.shortcuts.clone().into_any_element(),
      None if on_calendar => self.calendar.clone().into_any_element(),
      None if on_meet => self.meet.clone().into_any_element(),
      None if on_mail => self.mail.clone().into_any_element(),
      None => self
        .home
        .clone()
        .cached(StyleRefinement::default().size_full())
        .into_any_element(),
    };

    // Own bar instead of a fixed-height title bar: tall enough for the search field, draggable, and with the
    // same 80px reserved on both sides so the field sits in the middle of the window.
    let bar = h_flex()
      .id("top-bar")
      .w_full()
      .flex_shrink_0()
      // On macOS clear of the traffic lights, with a visible gap before the mark.
      .pl(px(chrome::inset(92.)))
      .pr_4()
      .gap_3()
      .items_center()
      .when(Self::sidebar_side(cx) == SidebarSide::Left, |bar| bar.child(Self::sidebar_button(cx)))
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
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(icon(Icon::Search, 15., MUTED))
            .child(div().flex_1().child(Input::new(&self.search).appearance(false).bordered(false))),
        ),
      );
    let group = h_flex()
      // Off macOS the kit's three 34px window buttons (102px) follow this group in the bar, so it reserves that much
      // less: the field stays centered, and at the window's minimum width the buttons stay inside it.
      .min_w(px(if cfg!(target_os = "macos") { 252. } else { 150. }))
      .flex_shrink_0()
      .gap_2()
      .items_center()
      .justify_end()
      .child(self.indicator(cx))
      .when(Self::sidebar_side(cx) == SidebarSide::Right, |group| {
        group.child(Self::sidebar_button(cx))
      });
    // Off macOS the system has no menu bar for the app: its menus are behind the bar's last button.
    #[cfg(not(target_os = "macos"))]
    let group = group.child(self.menu_button(cx));
    let bar = bar.child(group);

    let hidden = Self::sidebar_mode(cx) != SidebarMode::Open;
    let right = Self::sidebar_side(cx) == SidebarSide::Right;
    let columns = if hidden { Vec::new() } else { self.columns(cx) };
    let (before, after) = if right {
      (Vec::new(), columns)
    } else {
      (columns, Vec::new())
    };
    let overlay = (hidden && self.overlay.is_some()).then(|| self.overlay_element(cx));
    let handle = (!hidden).then(|| self.resize_handle(cx));
    // While the floating sidebar is open, Escape reaches `on_escape` with no text field in focus too (see `main.rs`).
    let context = if overlay.is_some() {
      "Root Overlay"
    } else {
      "Root"
    };
    // The cards in the bottom-right corner, stacked: the restart card, and on Linux the question about the apps menu.
    let cards: Vec<AnyElement> = crate::updater::card(cx).into_iter().collect();
    #[cfg(target_os = "linux")]
    let cards: Vec<AnyElement> = cards.into_iter().chain(crate::desktop_entry::card(cx)).collect();

    emoji_font::fallback(v_flex())
      .size_full()
      .bg(rgb(BG))
      .text_color(rgb(TEXT))
      .text_size(px(13.))
      .font_family(theme::sans())
      .key_context(context)
      .track_focus(&self.focus)
      .on_action(cx.listener(|this, _: &Escape, window, cx| this.on_escape(window, cx)))
      .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
        this.drag_column(event, cx);
        this.track_pointer(event.position, window, cx);
      }))
      .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_resize(cx)))
      .when(self.resizing.is_some(), |root| root.cursor(CursorStyle::ResizeLeftRight))
      .on_action(cx.listener(|this, _: &OpenSwitcher, window, cx| this.open_switcher(window, cx)))
      .on_action(cx.listener(|this, _: &OpenThemePicker, window, cx| this.open_theme_picker(window, cx)))
      .on_action(cx.listener(|this, _: &OpenLayoutPicker, window, cx| this.open_choice_picker::<Layout>(window, cx)))
      .on_action(cx.listener(|this, _: &OpenMessageStylePicker, window, cx| {
        this.open_choice_picker::<settings::MessageStyle>(window, cx);
      }))
      .on_action(cx.listener(|this, _: &OpenAbout, window, cx| this.open_about(window, cx)))
      .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
      .on_action(cx.listener(|this, _: &OpenNotificationSettings, window, cx| this.open_settings_at(true, window, cx)))
      .on_action(cx.listener(|this, _: &GoHome, window, cx| this.go_home(window, cx)))
      .on_action(cx.listener(|this, _: &OpenCalendar, window, cx| this.open_calendar(window, cx)))
      .on_action(cx.listener(|this, _: &OpenMeet, window, cx| this.open_meet(window, cx)))
      .on_action(cx.listener(|this, _: &OpenMail, window, cx| this.open_mail(window, cx)))
      .on_action(cx.listener(|this, _: &GoBack, window, cx| this.step(true, window, cx)))
      .on_action(cx.listener(|this, _: &GoForward, window, cx| this.step(false, window, cx)))
      .on_action(cx.listener(|this, _: &NextConversation, window, cx| this.step_conversation(true, window, cx)))
      .on_action(cx.listener(|this, _: &PreviousConversation, window, cx| this.step_conversation(false, window, cx)))
      .on_action(cx.listener(|this, _: &NextUnread, window, cx| this.open_next_unread(window, cx)))
      .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)))
      .on_action(cx.listener(|this, _: &OpenNewChat, window, cx| this.open_new_chat(window, cx)))
      .on_action(cx.listener(|this, _: &RewriteDraft, window, cx| this.rewrite_draft(window, cx)))
      .on_action(cx.listener(|this, _: &FindInChat, window, cx| this.find_in_chat(window, cx)))
      .on_action(cx.listener(|this, _: &FindNext, window, cx| this.find_next(window, cx)))
      .on_action(cx.listener(|this, _: &FindPrevious, window, cx| this.find_previous(window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey1, window, cx| this.open_hotkey(1, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey2, window, cx| this.open_hotkey(2, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey3, window, cx| this.open_hotkey(3, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey4, window, cx| this.open_hotkey(4, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey5, window, cx| this.open_hotkey(5, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey6, window, cx| this.open_hotkey(6, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey7, window, cx| this.open_hotkey(7, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey8, window, cx| this.open_hotkey(8, window, cx)))
      .on_action(cx.listener(|this, _: &Hotkey9, window, cx| this.open_hotkey(9, window, cx)))
      .child(chrome::top_bar(bar, TOP_BAR_HEIGHT))
      .child(
        h_flex()
          .relative()
          .flex_1()
          .min_h_0()
          .children(before)
          .child(
            div()
              .flex_1()
              .h_full()
              .min_w_0()
              // Nothing beside it on the sidebar's side: the margin the sidebar's rows would give.
              .when(hidden || right, gpui_kit::Styled::ml_2)
              .when(hidden || !right, gpui_kit::Styled::mr_2)
              .mb_2()
              .rounded(px(14.))
              .bg(rgb(PANEL))
              .overflow_hidden()
              .child(content),
          )
          .children(after)
          .children(handle)
          .children(overlay),
      )
      .children(self.switcher.as_ref().map(|(switcher, _)| switcher.clone()))
      .children(self.theme_picker.as_ref().map(|(picker, _)| picker.clone()))
      .children(self.choice_picker.as_ref().map(|picker| picker.view.clone()))
      .children(self.about.as_ref().map(|(about, _)| about.clone()))
      .children(self.new_chat.as_ref().map(|(view, _)| view.clone()))
      .children(self.thread_popup(window, cx))
      .children(self.settings.as_ref().map(|(view, _)| view.clone()))
      .children(self.calendar.read(cx).overlays())
      .child(self.viewer.clone())
      .child(self.composer.clone())
      .child(self.menu.clone())
      .child(self.text_panel.clone())
      .child(self.emoji.clone())
      .when(!cards.is_empty(), |root| {
        root.child(v_flex().absolute().right(px(20.)).bottom(px(20.)).gap_2().children(cards))
      })
      .into_any_element()
  }
}
