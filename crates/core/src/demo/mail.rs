//! The demo's Gmail: a made-up mailbox kept in memory, read-only like the real one. Threads are written to the
//! workspace's people and a few invented senders; times are relative to the moment the demo starts.

#![allow(clippy::unreadable_literal, reason = "colors are 0xRRGGBB")]

use std::collections::{HashMap, HashSet};

use super::image::{BEANS, HERO_DARK, Photo};
use crate::mail::SignaturePlace;
use crate::model::{
  MailAddress, MailAttachment, MailCompose, MailLabel, MailMessage, MailReply, MailSignature, MailThread, Mailbox,
  Micros, User,
};

/// Threads per page, as the engine asks Gmail for them.
pub const PAGE: usize = 25;
/// Longest snippet, in characters.
const SNIPPET: usize = 120;
/// Minutes per hour and per day, for `Msg::ago`.
const H: i64 = 60;
const D: i64 = 24 * H;

/// The signed-in user, wherever an address is written.
const ME: &str = "me";
const JORDAN: &str = "jpark@example.com";
const RILEY: &str = "rbrooks@example.com";
const SAM: &str = "sokafor@example.com";
const PRIYA: &str = "praman@example.com";
const DIEGO: &str = "dalvarez@example.com";
const MEI: &str = "mtanaka@example.com";
const NOAH: &str = "nfischer@example.com";
const FIELDNOTES: &str = "news@fieldnotes.example.org";
const ACME: &str = "billing@acmecloud.example.com";
const NORTHWIND: &str = "trips@northwind.example.com";
const LUMEN: &str = "digest@lumen.example.com";
const HARBORLINE: &str = "orders@harborline.example.com";
/// Where answers to Fieldnotes Weekly go.
const FIELDNOTES_EDITORS: &str = "editors@fieldnotes.example.org";

/// Senders from outside the workspace: (name, email).
const OUTSIDE: &[(&str, &str)] = &[
  ("Fieldnotes Weekly", FIELDNOTES),
  ("Fieldnotes editors", FIELDNOTES_EDITORS),
  ("Acme Cloud Billing", ACME),
  ("Northwind Travel", NORTHWIND),
  ("Lumen Analytics", LUMEN),
  ("Harborline Coffee", HARBORLINE),
];

/// Files the demo's emails carry, served at `demo:file/<name>`: (name, MIME type, bytes).
const FILES: &[(&str, &str, &str)] = &[
  (
    "q4-budget.csv",
    "text/csv",
    "Team,Q3 actual,Q4 plan\nDesign,42000,45000\nEngineering,180000,186000\nSupport,38000,41000\nEvents,12000,9500\n",
  ),
  (
    "offsite-agenda.txt",
    "text/plain",
    "Team offsite agenda (made up for the Working demo)\n\nDay 1\n09:30 Coffee and intros\n10:00 Year in review\n\
     13:00 Lunch\n14:00 Roadmap workshop\n\nDay 2\n09:30 Customer stories\n11:00 Planning next quarter\n\
     15:00 Wrap-up and travel\n",
  ),
  (
    "support-macros.md",
    "text/markdown",
    "# Launch week macros\n\n## Pricing question\nThanks for asking! The new plans are on the pricing page.\n\n\
     ## Bug report\nThanks for the report. We are looking into it and will write back within a day.\n",
  ),
  (
    "invoice-november.csv",
    "text/csv",
    "Service,Usage,Amount\nCompute,1240 hours,310.00\nStorage,820 GB,41.00\nNetwork,96 GB,8.64\nTotal,,359.64\n",
  ),
  (
    "brand-notes.txt",
    "text/plain",
    "Brand notes (made up for the Working demo)\n\n- Primary color: deep teal\n- Type: one sans-serif family, \
     two weights\n- Voice: plain, warm, short sentences\n",
  ),
];

/// The bytes of a file the demo's emails carry.
pub fn file(name: &str) -> Option<&'static [u8]> {
  FILES
    .iter()
    .find(|(file, ..)| *file == name)
    .map(|(_, _, body)| body.as_bytes())
}

/// Which lists a thread is in besides All (and Sent, which follows from its messages).
#[derive(Clone, Copy)]
struct Labels {
  inbox: bool,
  starred: bool,
}

const INBOX: Labels = Labels { inbox: true, starred: false };
const INBOX_STARRED: Labels = Labels { inbox: true, starred: true };
const ARCHIVED: Labels = Labels { inbox: false, starred: false };
const ARCHIVED_STARRED: Labels = Labels { inbox: false, starred: true };

/// How an email's HTML is made.
#[derive(Clone, Copy)]
enum Layout {
  /// From its text: blank lines between paragraphs, `- ` lines as a list, `https://` words as links.
  Text,
  /// The Fieldnotes newsletter's table layout.
  Newsletter,
  /// Paragraphs around an inline photo of the kitchen's coffee beans.
  Beans,
}

/// A file on an email.
enum Attached {
  /// One of `FILES`.
  File(&'static str),
  /// One of the demo's photos, under a name of its own.
  Photo(&'static Photo, &'static str),
}

/// An email as written here.
struct Msg {
  from: &'static str,
  to: &'static [&'static str],
  cc: &'static [&'static str],
  /// Where answers go instead of `from`.
  reply_to: &'static [&'static str],
  /// Minutes before the demo started.
  ago: i64,
  text: &'static str,
  unread: bool,
  attached: &'static [Attached],
  layout: Layout,
}

const MSG: Msg = Msg {
  from: ME,
  to: &[ME],
  cc: &[],
  reply_to: &[],
  ago: 0,
  text: "",
  unread: false,
  attached: &[],
  layout: Layout::Text,
};

/// A label of the user's: id, full name, color, and the ids of the threads that carry it (`Spec::id`).
#[derive(Clone, Copy)]
struct LabelSpec {
  id: &'static str,
  name: &'static str,
  color: Option<u32>,
  threads: &'static [&'static str],
}

const fn label(id: &'static str, name: &'static str, threads: &'static [&'static str]) -> LabelSpec {
  LabelSpec { id, name, color: None, threads }
}

const fn colored(color: u32, label: LabelSpec) -> LabelSpec {
  LabelSpec { color: Some(color), ..label }
}

const TEAM: &[&str] = &[
  "lunch-thursday",
  "offsite-agenda",
  "book-club",
  "photo-day",
  "holiday-coverage",
  "parking-passes",
  "on-call",
  "weekend-hike",
];
const LAUNCH: &[&str] = &[
  "launch-copy",
  "support-macros",
  "hero-images",
  "pricing-copy",
  "roadmap-notes",
  "accessibility-audit",
  "incoming-launch-email",
];
const DESIGN: &[&str] = &[
  "design-review",
  "accessibility-audit",
  "onboarding-mocks",
  "design-guild",
  "brand-notes",
];
const RECEIPTS: &[&str] = &["invoice-november", "invoice-october", "coffee-shipped", "subscription"];
const NEWSLETTERS: &[&str] = &["fieldnotes-112", "fieldnotes-111", "fieldnotes-110", "lumen-weekly"];
const ACME_CLOUD: &[&str] = &["invoice-november", "invoice-october", "maintenance"];
const TRAVEL_2026: &[&str] = &["trip-porto", "booking-confirmed", "check-in"];

/// The user's labels, in no particular order.
const LABELS: &[LabelSpec] = &[
  label("Label_1", "Team", TEAM),
  colored(0xe66550, label("Label_2", "Launch", LAUNCH)),
  colored(0x8e63ce, label("Label_3", "Design", DESIGN)),
  label("Label_4", "Finance", &["q4-budget", "travel-budget", "expenses"]),
  colored(0x43d692, label("Label_5", "Receipts", RECEIPTS)),
  label("Label_6", "Clients", &["brightwater-call", "staging-slow"]),
  colored(0x4a86e8, label("Label_7", "Clients/Brightwater", &["brightwater-call"])),
  label("Label_8", "Vendors", &["maintenance"]),
  colored(0xffad46, label("Label_9", "Vendors/Acme Cloud", ACME_CLOUD)),
  label("Label_10", "Vendors/Harborline", &["coffee-shipped", "subscription"]),
  label("Label_11", "Travel", &["check-in"]),
  colored(0x2da2bb, label("Label_12", "Travel/2026", TRAVEL_2026)),
  colored(0x999999, label("Label_13", "Newsletters", NEWSLETTERS)),
];

/// A thread as written here.
struct Spec {
  id: &'static str,
  subject: &'static str,
  labels: Labels,
  /// Oldest first.
  messages: &'static [Msg],
}

/// The one email that arrives while the demo runs, after the first inbox load.
const INCOMING: Spec = Spec {
  id: "incoming-launch-email",
  subject: "One more look at the launch email?",
  labels: INBOX,
  messages: &[Msg {
    from: JORDAN,
    cc: &[RILEY],
    text: "Hi Avery,\n\nCould you take one more look at the launch email before 3? I moved the pricing paragraph up \
           and shortened the intro.\n\nThe draft is here: https://docs.example.com/launch-email\n\nThanks,\nJordan",
    unread: true,
    ..MSG
  }],
};

/// The mailbox the demo starts with, in no particular order.
const THREADS: &[Spec] = &[
  Spec {
    id: "launch-copy",
    subject: "Launch email: final copy",
    labels: INBOX,
    messages: &[
      Msg {
        from: JORDAN,
        cc: &[RILEY],
        ago: 3 * H + 10,
        text: "Hi Avery,\n\nHere is where the launch email stands:\n\n- Subject line picked\n- Pricing paragraph \
               rewritten\n- Hero image swapped for the light one\n\nI will send the final copy after lunch.\n\nJordan",
        ..MSG
      },
      Msg {
        from: JORDAN,
        cc: &[RILEY],
        ago: 25,
        text: "Final copy is in the doc: https://docs.example.com/launch-email\n\nOne open question: do we mention \
               the annual discount in the first paragraph or keep it for the pricing page?\n\nJordan",
        unread: true,
        ..MSG
      },
    ],
  },
  Spec {
    id: "fieldnotes-112",
    subject: "Fieldnotes Weekly: shipping small, shipping often",
    labels: INBOX,
    messages: &[Msg {
      from: FIELDNOTES,
      ago: H,
      text: "Fieldnotes Weekly, issue 112: Shipping small, shipping often\n\nThis week: why teams that ship small \
             changes every day spend less time fixing them, and three habits that help.\n\nRead the essay: \
             https://fieldnotes.example.org/112\n\nAlso in this issue\n\nTools: a checklist for release notes people \
             actually read.\nInterview: a support lead on writing replies that close tickets.\n\nYou get this \
             because you subscribed at fieldnotes.example.org. Unsubscribe: https://fieldnotes.example.org/unsubscribe",
      unread: true,
      layout: Layout::Newsletter,
      reply_to: &[FIELDNOTES_EDITORS],
      ..MSG
    }],
  },
  Spec {
    id: "lunch-thursday",
    subject: "Lunch Thursday?",
    labels: INBOX,
    messages: &[
      Msg {
        from: RILEY,
        ago: 4 * H,
        text: "Ramen place on 5th, Thursday at 12:30? I can book a table for four.",
        ..MSG
      },
      Msg {
        to: &[RILEY],
        ago: 3 * H + 40,
        text: "Yes please! Count me in.",
        ..MSG
      },
      Msg {
        from: RILEY,
        ago: 2 * H + 5,
        text: "Booked for 12:30. Sam and Mei are coming too.",
        unread: true,
        ..MSG
      },
    ],
  },
  Spec {
    id: "invoice-november",
    subject: "Your November invoice is ready",
    labels: INBOX,
    messages: &[Msg {
      from: ACME,
      ago: 3 * H,
      text: "Your invoice for November is ready. Total: 359.64 USD, charged on December 1.\n\nThe details are \
             attached.\n\nAcme Cloud Billing",
      attached: &[Attached::File("invoice-november.csv")],
      ..MSG
    }],
  },
  Spec {
    id: "design-review",
    subject: "Design review notes",
    labels: INBOX_STARRED,
    messages: &[
      Msg {
        from: PRIYA,
        cc: &[MEI],
        ago: 5 * H + 30,
        text: "Hi Avery,\n\nNotes from today's design review are here: https://docs.example.com/design-review\n\n\
               The main decisions:\n\n- Keep the sidebar collapsible\n- Move search to the top bar\n- Ship dark mode \
               with the launch\n\nPriya",
        ..MSG
      },
      Msg {
        from: MEI,
        to: &[PRIYA, ME],
        ago: 4 * H + 15,
        text: "Thanks Priya. I added the contrast numbers for dark mode at the end of the doc.",
        ..MSG
      },
    ],
  },
  Spec {
    id: "support-macros",
    subject: "Support macros for launch week",
    labels: INBOX,
    messages: &[Msg {
      from: SAM,
      ago: 5 * H,
      text: "Hi Avery,\n\nI drafted the support macros for launch week. Could you check the pricing answer before \
             Friday?\n\nThanks,\nSam",
      unread: true,
      attached: &[Attached::File("support-macros.md")],
      ..MSG
    }],
  },
  Spec {
    id: "kitchen-beans",
    subject: "New beans in the kitchen",
    labels: INBOX,
    messages: &[Msg {
      from: MEI,
      ago: 7 * H,
      text: "New beans in the kitchen, a washed Ethiopian. Very bright.\n\nGrinder is set for pour-over.\n\nMei",
      layout: Layout::Beans,
      ..MSG
    }],
  },
  Spec {
    id: "q4-budget",
    subject: "Q4 budget draft",
    labels: INBOX_STARRED,
    messages: &[
      Msg {
        from: NOAH,
        to: &[ME, PRIYA],
        ago: D + 2 * H,
        text: "Hi both,\n\nAttached is the Q4 budget draft. Events went down because the offsite moved to January.\
               \n\nNoah",
        attached: &[Attached::File("q4-budget.csv")],
        ..MSG
      },
      Msg {
        to: &[NOAH, PRIYA],
        ago: D + H + 30,
        text: "Looks right to me. Can design keep the extra 3000 for the illustration contract?",
        ..MSG
      },
      Msg {
        from: PRIYA,
        to: &[NOAH, ME],
        ago: D + H,
        text: "Yes, the illustration contract is the reason for the bump.",
        ..MSG
      },
      Msg {
        from: NOAH,
        to: &[ME, PRIYA],
        ago: D + 20,
        text: "Great, I will send it to finance tomorrow morning.",
        ..MSG
      },
    ],
  },
  Spec {
    id: "staging-slow",
    subject: "Staging is slow again",
    labels: INBOX,
    messages: &[
      Msg {
        to: &[DIEGO],
        ago: D + 4 * H,
        text: "Hi Diego,\n\nStaging takes about ten seconds to load the dashboard this morning. Anything going on?",
        ..MSG
      },
      Msg {
        from: DIEGO,
        ago: D + 3 * H,
        text: "A migration was running on the staging database. It finished ten minutes ago, so it should be quick \
               again.\n\nDiego",
        ..MSG
      },
    ],
  },
  Spec {
    id: "trip-porto",
    subject: "Your trip to Porto: itinerary",
    labels: INBOX_STARRED,
    messages: &[Msg {
      from: NORTHWIND,
      ago: D + 6 * H,
      text: "Your itinerary\n\n- Flight out: December 9, 08:15\n- Hotel: Rua das Flores, 3 nights\n- Flight back: \
             December 12, 18:40\n\nManage your trip: https://northwind.example.com/trips\n\nNorthwind Travel",
      ..MSG
    }],
  },
  Spec {
    id: "offsite-agenda",
    subject: "Offsite agenda",
    labels: INBOX,
    messages: &[Msg {
      from: RILEY,
      to: &[ME, JORDAN, SAM],
      ago: 2 * D + H,
      text: "Hi all,\n\nThe agenda for the offsite is attached. Tell me by Friday if you want a slot for your team.\
             \n\nRiley",
      attached: &[Attached::File("offsite-agenda.txt")],
      ..MSG
    }],
  },
  Spec {
    id: "lumen-weekly",
    subject: "Your weekly dashboard summary",
    labels: INBOX,
    messages: &[Msg {
      from: LUMEN,
      ago: 2 * D + 4 * H,
      text: "This week on your dashboard\n\n- Visitors: up 12 percent\n- Sign-ups: up 8 percent\n- Most viewed page: \
             Pricing\n\nOpen the dashboard: https://lumen.example.com/dashboard",
      unread: true,
      ..MSG
    }],
  },
  Spec {
    id: "hero-images",
    subject: "Hero images for the landing page",
    labels: INBOX,
    messages: &[Msg {
      from: JORDAN,
      ago: 2 * D + 8 * H,
      text: "The dark hero is attached. The light one is already on staging.",
      attached: &[Attached::Photo(&HERO_DARK, "hero-dark.jpg")],
      ..MSG
    }],
  },
  Spec {
    id: "interview-loop",
    subject: "Interview loop next week",
    labels: INBOX,
    messages: &[Msg {
      from: PRIYA,
      ago: 3 * D,
      text: "Hi Avery,\n\nCould you take the product sense interview on Tuesday at 2? The candidate's portfolio is in \
             the hiring folder.\n\nPriya",
      ..MSG
    }],
  },
  Spec {
    id: "coffee-shipped",
    subject: "Your order has shipped",
    labels: INBOX,
    messages: &[Msg {
      from: HARBORLINE,
      ago: 3 * D + 5 * H,
      text: "Your order of two bags of house blend has shipped and should arrive on Thursday.\n\nHarborline Coffee",
      ..MSG
    }],
  },
  Spec {
    id: "brightwater-call",
    subject: "Customer call recap: Brightwater",
    labels: INBOX,
    messages: &[
      Msg {
        from: SAM,
        to: &[ME, JORDAN],
        ago: 4 * D + 3 * H,
        text: "Recap from the Brightwater call:\n\n- They want single sign-on before rolling out to every team\n\
               - Exports to CSV are a must\n- Renewal is in February\n\nSam",
        ..MSG
      },
      Msg {
        from: JORDAN,
        to: &[SAM, ME],
        ago: 4 * D + 2 * H,
        text: "Single sign-on is on the roadmap for January. I can join the next call.",
        ..MSG
      },
      Msg {
        to: &[SAM, JORDAN],
        ago: 4 * D + H,
        text: "Thanks Sam. CSV exports are already in the beta, I will send them the link.",
        ..MSG
      },
    ],
  },
  Spec {
    id: "parking-passes",
    subject: "Parking passes",
    labels: INBOX,
    messages: &[Msg {
      from: NOAH,
      ago: 4 * D + 6 * H,
      text: "New parking passes are at the front desk. Bring your badge to pick yours up.",
      ..MSG
    }],
  },
  Spec {
    id: "accessibility-audit",
    subject: "Accessibility audit results",
    labels: INBOX,
    messages: &[Msg {
      from: MEI,
      to: &[ME, PRIYA],
      ago: 5 * D,
      text: "The audit is done. Three things to fix before launch:\n\n- Focus ring missing on the settings tabs\n\
             - Two icons without labels\n- Low contrast on disabled buttons\n\nEverything else passed.\n\nMei",
      ..MSG
    }],
  },
  Spec {
    id: "on-call",
    subject: "On-call handoff",
    labels: INBOX,
    messages: &[Msg {
      from: DIEGO,
      ago: 5 * D + 4 * H,
      text: "Quiet week. One alert on Tuesday for disk space, cleaned up the old logs. You are on call from Monday.\
             \n\nDiego",
      ..MSG
    }],
  },
  Spec {
    id: "fieldnotes-111",
    subject: "Fieldnotes Weekly: the case for boring tech",
    labels: INBOX,
    messages: &[Msg {
      from: FIELDNOTES,
      ago: 7 * D,
      text: "Fieldnotes Weekly, issue 111\n\nThe case for boring tech: the tools you already know are the ones you \
             can fix at 2 in the morning.\n\nRead it: https://fieldnotes.example.org/111",
      ..MSG
    }],
  },
  Spec {
    id: "maintenance",
    subject: "Scheduled maintenance on December 2",
    labels: INBOX,
    messages: &[Msg {
      from: ACME,
      ago: 8 * D,
      text: "We will update the storage service on December 2 between 02:00 and 03:00 UTC. No downtime is expected.\
             \n\nAcme Cloud",
      ..MSG
    }],
  },
  Spec {
    id: "book-club",
    subject: "Book club pick for December",
    labels: INBOX,
    messages: &[
      Msg {
        from: RILEY,
        to: &[ME, MEI],
        ago: 9 * D + 2 * H,
        text: "Two picks for December: a short novel or a book about bridges. Votes by Friday!",
        ..MSG
      },
      Msg {
        from: MEI,
        to: &[RILEY, ME],
        ago: 9 * D,
        text: "Bridges, obviously.",
        ..MSG
      },
    ],
  },
  Spec {
    id: "pricing-copy",
    subject: "Pricing page copy",
    labels: INBOX,
    messages: &[Msg {
      from: JORDAN,
      ago: 10 * D,
      text: "First pass at the pricing page copy is in the doc: https://docs.example.com/pricing-copy\n\nComments \
             welcome.",
      ..MSG
    }],
  },
  Spec {
    id: "design-guild",
    subject: "Welcome to the design guild",
    labels: INBOX,
    messages: &[Msg {
      from: PRIYA,
      ago: 12 * D,
      text: "Welcome! The guild meets every other Wednesday at 4. Bring one thing you made and one thing you liked.\
             \n\nPriya",
      ..MSG
    }],
  },
  Spec {
    id: "check-in",
    subject: "Check in for your flight",
    labels: INBOX,
    messages: &[Msg {
      from: NORTHWIND,
      ago: 13 * D,
      text: "Check-in is open for your flight to Denver. Check in: https://northwind.example.com/check-in",
      ..MSG
    }],
  },
  Spec {
    id: "holiday-coverage",
    subject: "Holiday coverage",
    labels: INBOX,
    messages: &[
      Msg {
        from: SAM,
        ago: 15 * D + 4 * H,
        text: "Can you cover support on December 24 in the morning? I will take the 31st.",
        ..MSG
      },
      Msg {
        to: &[SAM],
        ago: 15 * D + 2 * H,
        text: "Deal. Morning of the 24th is mine.",
        ..MSG
      },
    ],
  },
  Spec {
    id: "expenses",
    subject: "Expense report reminder",
    labels: INBOX,
    messages: &[Msg {
      from: NOAH,
      ago: 18 * D,
      text: "Reminder: expense reports for November are due on the 5th.",
      ..MSG
    }],
  },
  Spec {
    id: "photo-day",
    subject: "Team photo day",
    labels: INBOX,
    messages: &[Msg {
      from: MEI,
      to: &[ME, JORDAN, RILEY, SAM],
      ago: 21 * D,
      text: "Team photos are on Tuesday at 11 on the roof. Wear something that is not black, please.",
      ..MSG
    }],
  },
  Spec {
    id: "roadmap-notes",
    subject: "Notes from the roadmap sync",
    labels: ARCHIVED,
    messages: &[Msg {
      to: &[JORDAN],
      ago: 6 * H,
      text: "Hi Jordan,\n\nMy notes from the roadmap sync:\n\n- Single sign-on moves to January\n- CSV exports leave \
             beta with the launch\n- Mobile stays read-only this quarter\n\nAvery",
      ..MSG
    }],
  },
  Spec {
    id: "onboarding-mocks",
    subject: "Mock-ups for onboarding",
    labels: ARCHIVED,
    messages: &[Msg {
      to: &[PRIYA, MEI],
      ago: D + 2 * H,
      text: "Hi both,\n\nThe onboarding mock-ups are here: https://design.example.com/onboarding\n\nI would love your \
             eyes on the second step before Thursday.\n\nAvery",
      ..MSG
    }],
  },
  Spec {
    id: "travel-budget",
    subject: "Travel budget",
    labels: ARCHIVED,
    messages: &[
      Msg {
        from: NOAH,
        ago: 3 * D + 2 * H,
        text: "How much travel should I plan for your team next quarter?",
        ..MSG
      },
      Msg {
        to: &[NOAH],
        ago: 3 * D,
        text: "Two trips: the offsite in January and one customer visit in March.",
        ..MSG
      },
    ],
  },
  Spec {
    id: "staging-access",
    subject: "Access to the staging dashboard",
    labels: ARCHIVED,
    messages: &[Msg {
      to: &[DIEGO],
      ago: 6 * D,
      text: "Could you give Mei access to the staging dashboard? She is running the accessibility audit.",
      ..MSG
    }],
  },
  Spec {
    id: "thank-you",
    subject: "Thank you!",
    labels: ARCHIVED,
    messages: &[Msg {
      to: &[SAM],
      ago: 11 * D,
      text: "Thanks for covering the support queue last week, Sam. It made the release so much calmer.",
      ..MSG
    }],
  },
  Spec {
    id: "weekend-hike",
    subject: "Weekend hike?",
    labels: ARCHIVED_STARRED,
    messages: &[
      Msg {
        to: &[RILEY],
        ago: 16 * D + 3 * H,
        text: "Hike on Saturday? The ridge trail should be dry by now.",
        ..MSG
      },
      Msg {
        from: RILEY,
        ago: 16 * D + H,
        text: "Yes! Meet at the trailhead at 8?",
        ..MSG
      },
    ],
  },
  Spec {
    id: "fieldnotes-110",
    subject: "Fieldnotes Weekly: notes on naming",
    labels: ARCHIVED,
    messages: &[Msg {
      from: FIELDNOTES,
      ago: 14 * D,
      text: "Fieldnotes Weekly, issue 110\n\nNotes on naming: a good name says what a thing does, not how it does it.\
             \n\nRead it: https://fieldnotes.example.org/110",
      ..MSG
    }],
  },
  Spec {
    id: "booking-confirmed",
    subject: "Your booking is confirmed",
    labels: ARCHIVED_STARRED,
    messages: &[Msg {
      from: NORTHWIND,
      ago: 20 * D,
      text: "Your booking to Porto is confirmed. Booking reference: NW4821.\n\nNorthwind Travel",
      ..MSG
    }],
  },
  Spec {
    id: "brand-notes",
    subject: "Old brand guidelines",
    labels: ARCHIVED,
    messages: &[Msg {
      from: PRIYA,
      ago: 25 * D,
      text: "The old brand notes, in case you need them for the archive page.",
      unread: true,
      attached: &[Attached::File("brand-notes.txt")],
      ..MSG
    }],
  },
  Spec {
    id: "invoice-october",
    subject: "Your October invoice is ready",
    labels: ARCHIVED,
    messages: &[Msg {
      from: ACME,
      ago: 30 * D,
      text: "Your invoice for October is ready. Total: 341.20 USD.\n\nAcme Cloud Billing",
      ..MSG
    }],
  },
  Spec {
    id: "subscription",
    subject: "Subscription renewed",
    labels: ARCHIVED,
    messages: &[Msg {
      from: HARBORLINE,
      ago: 35 * D,
      text: "Your coffee subscription renewed: two bags every month.",
      ..MSG
    }],
  },
  Spec {
    id: "welcome",
    subject: "Welcome aboard, Avery",
    labels: ARCHIVED_STARRED,
    messages: &[Msg {
      from: JORDAN,
      ago: 40 * D,
      text: "Welcome aboard, Avery!\n\nA few things for your first week:\n\n- Your laptop is at the front desk\n\
             - The team lunch is on Wednesday\n- The handbook: https://handbook.example.com\n\nGlad you are here.\n\
             Jordan",
      ..MSG
    }],
  },
];

/// A thread as the demo holds it.
struct Held {
  id: String,
  subject: String,
  labels: Labels,
  /// The ids of the user's labels on it.
  label_ids: Vec<&'static str>,
  /// Oldest first.
  messages: Vec<MailMessage>,
}

impl Held {
  fn time(&self) -> Micros {
    self.messages.last().map_or(0, |message| message.time)
  }

  fn unread(&self) -> bool {
    self.messages.iter().any(|message| message.unread)
  }

  fn summary(&self) -> MailThread {
    let mut senders: Vec<MailAddress> = Vec::new();
    for message in &self.messages {
      if !senders.iter().any(|sender| sender.email == message.from.email) {
        senders.push(message.from.clone());
      }
    }
    MailThread {
      id: self.id.clone(),
      subject: self.subject.clone(),
      snippet: self.messages.last().map_or_default(|message| snippet(&message.text)),
      senders,
      time: self.time(),
      messages: u32::try_from(self.messages.len()).unwrap_or(u32::MAX),
      unread: self.unread(),
      starred: self.labels.starred,
      attachments: self.messages.iter().any(|message| !message.attachments.is_empty()),
    }
  }

  /// Whether a Gmail search finds it: every word of the query, case-insensitive, in its subject, a sender's name or
  /// address, or a message's text.
  fn found(&self, query: &str) -> bool {
    let subject = self.subject.to_lowercase();
    let senders: Vec<String> = self
      .messages
      .iter()
      .map(|message| format!("{} {}", message.from.name, message.from.email).to_lowercase())
      .collect();
    let texts: Vec<String> = self.messages.iter().map(|message| message.text.to_lowercase()).collect();
    query.to_lowercase().split_whitespace().all(|word| {
      subject.contains(word)
        || senders.iter().any(|sender| sender.contains(word))
        || texts.iter().any(|text| text.contains(word))
    })
  }

  fn is_in(&self, mailbox: &Mailbox, me: &str) -> bool {
    match mailbox {
      Mailbox::Inbox => self.labels.inbox,
      Mailbox::Starred => self.labels.starred,
      Mailbox::Sent => self.messages.iter().any(|message| message.from.email == me),
      Mailbox::All => true,
      Mailbox::Label(id) => self.label_ids.contains(&id.as_str()),
      Mailbox::Search(query) => self.found(query),
    }
  }
}

/// The demo's mailbox.
pub struct DemoMail {
  me: MailAddress,
  /// Names of the workspace's people and the outside senders, by email.
  names: HashMap<String, String>,
  /// Newest first.
  threads: Vec<Held>,
  /// How many threads each list holds, by the pages loaded so far.
  held: HashMap<Mailbox, usize>,
  /// The inbox was loaded, so new mail is watched for.
  watching: bool,
  /// The one new thread the demo delivers has arrived.
  arrived: bool,
  /// Emails of the workspace's people, who answer back.
  people: HashSet<String>,
  /// Threads someone already answered back in, after a reply.
  answered: HashSet<String>,
}

/// What the app holds of the mailbox, to tell what a change touched: each loaded list, and while the inbox is
/// watched, the unread count and the labels.
#[derive(PartialEq, Eq)]
pub struct Snapshot {
  /// Each loaded list's threads and whether it has more.
  pub pages: HashMap<Mailbox, (Vec<MailThread>, bool)>,
  /// Unread inbox threads, while the inbox is watched.
  pub unread: Option<u32>,
  /// The labels, while the inbox is watched.
  pub labels: Option<Vec<MailLabel>>,
}

impl DemoMail {
  /// The mailbox of `me`, with threads from the workspace's `users` and a few outside senders.
  pub fn new(now: Micros, users: &[User], me: &User) -> Self {
    let mut names: HashMap<String, String> = users
      .iter()
      .filter(|user| !user.email.is_empty())
      .map(|user| (user.email.clone(), user.name.clone()))
      .collect();
    names.extend(OUTSIDE.iter().map(|(name, email)| ((*email).to_string(), (*name).to_string())));
    let people = users
      .iter()
      .filter(|user| !user.email.is_empty() && user.email != me.email)
      .map(|user| user.email.clone())
      .collect();
    let mut mail = Self {
      me: MailAddress {
        name: me.name.clone(),
        email: me.email.clone(),
      },
      names,
      threads: Vec::new(),
      held: HashMap::new(),
      watching: false,
      arrived: false,
      people,
      answered: HashSet::new(),
    };
    let threads = THREADS.iter().map(|spec| mail.build(spec, now)).collect();
    mail.threads = threads;
    mail.threads.sort_by_key(|thread| std::cmp::Reverse(thread.time()));
    mail
  }

  fn who(&self, email: &str) -> MailAddress {
    if email == ME {
      return self.me.clone();
    }
    MailAddress {
      name: self.names.get(email).cloned().unwrap_or_default(),
      email: email.into(),
    }
  }

  fn build(&self, spec: &Spec, now: Micros) -> Held {
    let id = format!("demo-mail-{}", spec.id);
    let messages = spec
      .messages
      .iter()
      .enumerate()
      .map(|(index, msg)| MailMessage {
        id: format!("{id}-{index}"),
        from: self.who(msg.from),
        to: msg.to.iter().map(|email| self.who(email)).collect(),
        cc: msg.cc.iter().map(|email| self.who(email)).collect(),
        bcc: Vec::new(),
        reply_to: msg.reply_to.iter().map(|email| self.who(email)).collect(),
        time: now - msg.ago * 60_000_000,
        html: match msg.layout {
          Layout::Text => html_of(msg.text),
          Layout::Newsletter => NEWSLETTER.into(),
          Layout::Beans => beans_html(),
        },
        text: msg.text.into(),
        unread: msg.unread,
        draft: false,
        attachments: msg.attached.iter().filter_map(attachment).collect(),
      })
      .collect();
    Held {
      id,
      subject: spec.subject.into(),
      labels: spec.labels,
      label_ids: LABELS
        .iter()
        .filter(|label| label.threads.contains(&spec.id))
        .map(|label| label.id)
        .collect(),
      messages,
    }
  }

  /// The first inbox load since the start or the last `stop` starts the watch: true only that once.
  pub const fn watch(&mut self) -> bool {
    let first = !self.watching;
    self.watching = true;
    first
  }

  /// Stops watching and forgets the pages held, so the next load starts fresh. The new email, once delivered,
  /// stays in the mailbox; until then the next inbox load schedules it again.
  pub fn stop(&mut self) {
    self.watching = false;
    self.held.clear();
  }

  /// A mailbox's newest page, or with `more` one more page than it holds, and whether older threads remain.
  pub fn load(&mut self, mailbox: &Mailbox, more: bool) -> (Vec<MailThread>, bool) {
    let total = self
      .threads
      .iter()
      .filter(|thread| thread.is_in(mailbox, &self.me.email))
      .count();
    let held = self.held.entry(mailbox.clone()).or_insert(0);
    let wanted = if more { *held + PAGE } else { PAGE };
    *held = wanted.min(total);
    self.page(mailbox)
  }

  /// The threads a mailbox holds now, newest first, and whether older threads remain.
  pub fn page(&self, mailbox: &Mailbox) -> (Vec<MailThread>, bool) {
    let held = self.held.get(mailbox).copied().unwrap_or(0);
    let matching: Vec<&Held> = self
      .threads
      .iter()
      .filter(|thread| thread.is_in(mailbox, &self.me.email))
      .collect();
    let threads = matching.iter().take(held).map(|thread| thread.summary()).collect();
    (threads, matching.len() > held)
  }

  /// A thread's emails, oldest first.
  pub fn open(&self, id: &str) -> Option<Vec<MailMessage>> {
    self
      .threads
      .iter()
      .find(|thread| thread.id == id)
      .map(|thread| thread.messages.clone())
  }

  /// Unread threads in the inbox.
  pub fn unread(&self) -> u32 {
    let count = self
      .threads
      .iter()
      .filter(|thread| thread.labels.inbox && thread.unread())
      .count();
    u32::try_from(count).unwrap_or(u32::MAX)
  }

  /// The user's labels, sorted by name ignoring case, each with how many of its threads are unread.
  pub fn labels(&self) -> Vec<MailLabel> {
    let mut labels: Vec<MailLabel> = LABELS
      .iter()
      .map(|label| {
        let unread = self
          .threads
          .iter()
          .filter(|thread| thread.unread() && thread.label_ids.contains(&label.id))
          .count();
        MailLabel {
          id: label.id.into(),
          name: label.name.into(),
          color: label.color,
          unread: u32::try_from(unread).unwrap_or(u32::MAX),
        }
      })
      .collect();
    labels.sort_by_key(|label| label.name.to_lowercase());
    labels
  }

  /// Delivers the demo's one new email at `now`: an unread inbox thread, newest of all, which the inbox list then
  /// holds on top of what it held. `None` once it has arrived.
  pub fn arrive(&mut self, now: Micros) -> Option<MailThread> {
    if self.arrived {
      return None;
    }
    self.arrived = true;
    let shown = self.shown();
    let held = self.build(&INCOMING, now);
    let summary = held.summary();
    self.threads.insert(0, held);
    self.refit(&shown);
    Some(summary)
  }

  /// Each loaded list, the unread count, and the labels, as the app holds them now.
  pub fn snapshot(&self) -> Snapshot {
    Snapshot {
      pages: self.held.keys().map(|mailbox| (mailbox.clone(), self.page(mailbox))).collect(),
      unread: self.watching.then(|| self.unread()),
      labels: self.watching.then(|| self.labels()),
    }
  }

  /// The ids each loaded list shows, and whether it has more.
  fn shown(&self) -> HashMap<Mailbox, (Vec<String>, bool)> {
    self
      .held
      .keys()
      .map(|mailbox| {
        let (threads, more) = self.page(mailbox);
        (mailbox.clone(), (threads.into_iter().map(|thread| thread.id).collect(), more))
      })
      .collect()
  }

  /// After a change, keeps every list showing what it showed (less what left it) plus what came in above: a list
  /// that showed everything still does, and one that pages reaches down to the oldest thread it showed.
  fn refit(&mut self, shown: &HashMap<Mailbox, (Vec<String>, bool)>) {
    let me = &self.me.email;
    let threads = &self.threads;
    for (mailbox, held) in &mut self.held {
      let Some((ids, more)) = shown.get(mailbox) else {
        continue;
      };
      let matching: Vec<&str> = threads
        .iter()
        .filter(|thread| thread.is_in(mailbox, me))
        .map(|thread| thread.id.as_str())
        .collect();
      *held = if *more {
        matching
          .iter()
          .rposition(|id| ids.iter().any(|shown| shown == id))
          .map_or(0, |last| last + 1)
      } else {
        matching.len()
      };
    }
  }

  /// Marks every email of a thread read, or its newest one unread. False when there is no such thread.
  pub fn mark(&mut self, id: &str, read: bool) -> bool {
    let Some(thread) = self.threads.iter_mut().find(|thread| thread.id == id) else {
      return false;
    };
    if read {
      for message in &mut thread.messages {
        message.unread = false;
      }
    } else if let Some(message) = thread.messages.last_mut() {
      message.unread = true;
    }
    true
  }

  /// Takes a thread out of the inbox, or puts it back. False when there is no such thread.
  pub fn archive(&mut self, id: &str, archive: bool) -> bool {
    let shown = self.shown();
    let Some(thread) = self.threads.iter_mut().find(|thread| thread.id == id) else {
      return false;
    };
    thread.labels.inbox = !archive;
    self.refit(&shown);
    true
  }

  /// Avery's made-up signatures: a full one for new emails and a shorter one for replies.
  pub fn signatures(&self) -> (Option<MailSignature>, Option<MailSignature>) {
    let first = self.me.name.split_whitespace().next().unwrap_or_default();
    let email = escape(&self.me.email);
    let new = format!(
      "<div dir=\"ltr\"><b>{}</b><br>Product designer, Working demo<br><a href=\"mailto:{email}\">{email}</a></div>",
      escape(&self.me.name)
    );
    let reply = format!("<div dir=\"ltr\">{} · Product design</div>", escape(first));
    let signature = |html: String| MailSignature {
      text: crate::mail::html_to_text(&html),
      html,
    };
    (Some(signature(new)), Some(signature(reply)))
  }

  /// Appends the user's answer to a thread at `now`, the answered email quoted under it, and moves the thread to the
  /// top. Answers the thread's emails and, when the thread's last sender was one of the workspace's people who has
  /// not answered back in it yet, that person's email. `None` when there is no such thread.
  pub fn reply(&mut self, reply: &MailReply, now: Micros) -> Option<(Vec<MailMessage>, Option<String>)> {
    let shown = self.shown();
    let index = self.threads.iter().position(|thread| thread.id == reply.thread)?;
    let mut thread = self.threads.remove(index);
    let last = thread.messages.last().map_or_default(|message| message.from.email.clone());
    let mut html = html_of(&reply.text);
    if let Some(quoted) = thread.messages.iter().find(|message| message.id == reply.message) {
      html.push_str("<blockquote class=\"gmail_quote\">");
      html.push_str(&quoted.html);
      html.push_str("</blockquote>");
    }
    // As Gmail's default: the reply signature after the quoted email.
    let signed = reply.signature.then(|| self.signatures().1).flatten();
    if let Some(signature) = &signed {
      html.push_str(&crate::mail::signature_block(&signature.html, SignaturePlace::AfterQuote));
    }
    let text = signed.as_ref().map_or_else(
      || reply.text.clone(),
      |signature| format!("{}\n\n-- \n{}", reply.text.trim_end(), signature.text),
    );
    thread.messages.push(MailMessage {
      id: format!("{}-{}", thread.id, thread.messages.len()),
      from: self.me.clone(),
      to: reply.to.clone(),
      cc: reply.cc.clone(),
      bcc: Vec::new(),
      reply_to: Vec::new(),
      time: now,
      html,
      text,
      unread: false,
      draft: false,
      attachments: Vec::new(),
    });
    let messages = thread.messages.clone();
    self.threads.insert(0, thread);
    self.refit(&shown);
    let answers = self.people.contains(&last) && self.answered.insert(reply.thread.clone());
    Some((messages, answers.then_some(last)))
  }

  /// The user's new email at `now`: a thread of its own on top, in Sent, and in the inbox, read, when the user is
  /// among its recipients. Answers the thread's id and, when its first recipient is one of the workspace's people,
  /// that person, who answers back once.
  pub fn compose(&mut self, compose: &MailCompose, now: Micros) -> (String, Option<String>) {
    let shown = self.shown();
    let id = format!(
      "composed-{}",
      self.threads.iter().filter(|thread| thread.id.starts_with("composed-")).count()
    );
    let signed = compose.signature.then(|| self.signatures().0).flatten();
    let (html, text) = signed.as_ref().map_or_else(
      || (html_of(&compose.text), compose.text.clone()),
      |signature| {
        (
          html_of(&compose.text)
            + &crate::mail::signature_block(&signature.html, SignaturePlace::new_email(&compose.text)),
          format!("{}\n\n-- \n{}", compose.text.trim_end(), signature.text),
        )
      },
    );
    let to_me = compose
      .to
      .iter()
      .chain(&compose.cc)
      .any(|address| address.email.eq_ignore_ascii_case(&self.me.email));
    self.threads.insert(
      0,
      Held {
        id: id.clone(),
        subject: compose.subject.trim().to_string(),
        labels: Labels { inbox: to_me, starred: false },
        label_ids: Vec::new(),
        messages: vec![MailMessage {
          id: format!("{id}-0"),
          from: self.me.clone(),
          to: compose.to.clone(),
          cc: compose.cc.clone(),
          bcc: compose.bcc.clone(),
          reply_to: Vec::new(),
          time: now,
          html,
          text,
          unread: false,
          draft: false,
          attachments: Vec::new(),
        }],
      },
    );
    self.refit(&shown);
    let first = compose
      .to
      .first()
      .or_else(|| compose.cc.first())
      .map(|address| address.email.to_lowercase());
    let answers = first.filter(|email| self.people.contains(email) && self.answered.insert(id.clone()));
    (id, answers)
  }

  /// `from` answers the user's reply in a thread at `now`: a short unread email that brings the thread back to the
  /// top of the inbox. `None` when there is no such thread.
  pub fn answer(&mut self, id: &str, from: &str, now: Micros) -> Option<MailThread> {
    let shown = self.shown();
    let index = self.threads.iter().position(|thread| thread.id == id)?;
    let mut thread = self.threads.remove(index);
    let sender = self.who(from);
    let me = self.me.name.split_whitespace().next().unwrap_or_default();
    let name = sender.name.split_whitespace().next().unwrap_or_default();
    let text = match thread.messages.len() % 3 {
      0 => format!("Thanks, {me}! That works for me.\n\n{name}"),
      1 => format!("Perfect, thank you {me}. I will take it from here.\n\n{name}"),
      _ => format!("Got it, thanks! I will follow up tomorrow.\n\n{name}"),
    };
    thread.messages.push(MailMessage {
      id: format!("{}-{}", thread.id, thread.messages.len()),
      from: sender,
      to: vec![self.me.clone()],
      cc: Vec::new(),
      bcc: Vec::new(),
      reply_to: Vec::new(),
      time: now,
      html: html_of(&text),
      text,
      unread: true,
      draft: false,
      attachments: Vec::new(),
    });
    thread.labels.inbox = true;
    let summary = thread.summary();
    self.threads.insert(0, thread);
    self.refit(&shown);
    Some(summary)
  }
}

fn attachment(attached: &Attached) -> Option<MailAttachment> {
  let (name, mime, size, url) = match attached {
    Attached::File(name) => {
      let (file, mime, body) = FILES.iter().find(|(file, ..)| file == name)?;
      (*file, *mime, body.len(), format!("demo:file/{file}"))
    },
    Attached::Photo(photo, name) => (*name, "image/jpeg", photo.bytes.len(), photo.url()),
  };
  Some(MailAttachment {
    name: name.into(),
    mime: mime.into(),
    size: u64::try_from(size).unwrap_or(u64::MAX),
    url,
  })
}

fn escape(text: &str) -> String {
  text
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
}

/// One line of text as HTML, its `https://` words as links.
fn inline(line: &str) -> String {
  line
    .split(' ')
    .map(|word| {
      let word = escape(word);
      if word.starts_with("https://") {
        format!("<a href=\"{word}\">{word}</a>")
      } else {
        word
      }
    })
    .collect::<Vec<_>>()
    .join(" ")
}

/// HTML made from text as Gmail's compose writes it: each line its own `<div>`, an empty line `<div><br></div>`
/// between blocks, `- ` lines as a list.
fn html_of(text: &str) -> String {
  let mut html = String::from("<div dir=\"ltr\">");
  for (index, block) in text.trim().split("\n\n").enumerate() {
    if index > 0 {
      html.push_str("<div><br></div>");
    }
    if block.lines().all(|line| line.starts_with("- ")) {
      html.push_str("<ul>");
      for line in block.lines() {
        html.push_str("<li>");
        html.push_str(&inline(line.strip_prefix("- ").unwrap_or(line)));
        html.push_str("</li>");
      }
      html.push_str("</ul>");
    } else {
      for line in block.lines() {
        html.push_str("<div>");
        html.push_str(&inline(line));
        html.push_str("</div>");
      }
    }
  }
  html.push_str("</div>");
  html
}

/// Mei's note about the coffee beans, with the photo inline.
fn beans_html() -> String {
  format!(
    "<div dir=\"ltr\"><p>New beans in the kitchen, a washed Ethiopian. Very bright.</p><p><img src=\"{}\" \
     alt=\"Coffee beans on the kitchen counter\" width=\"576\" height=\"432\"></p><p>Grinder is set for pour-over.\
     </p><p>Mei</p></div>",
    BEANS.url()
  )
}

/// The start of a text, on one line.
fn snippet(text: &str) -> String {
  let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
  if line.chars().count() <= SNIPPET {
    return line;
  }
  let cut: String = line.chars().take(SNIPPET).collect();
  format!("{}…", cut.trim_end())
}

/// Fieldnotes Weekly's table layout, saying what its text says.
const NEWSLETTER: &str = "<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" \
  style=\"background:#f4f1ea;font-family:Georgia,serif\"><tr><td align=\"center\" style=\"padding:24px\">\
  <table role=\"presentation\" width=\"600\" cellpadding=\"0\" cellspacing=\"0\" style=\"background:#ffffff\">\
  <tr><td colspan=\"2\" style=\"padding:24px 32px;border-bottom:1px solid #e5e0d5\">\
  <h1 style=\"margin:0;font-size:24px;color:#2b2a27\">Fieldnotes Weekly</h1>\
  <p style=\"margin:4px 0 0;color:#8a857a;font-size:13px\">Issue 112: Shipping small, shipping often</p></td></tr>\
  <tr><td colspan=\"2\" style=\"padding:24px 32px;color:#2b2a27;font-size:16px;line-height:1.5\">\
  <p>This week: why teams that ship small changes every day spend less time fixing them, and three habits that \
  help.</p><p><a href=\"https://fieldnotes.example.org/112\" style=\"color:#b5542f\">Read the essay</a></p>\
  </td></tr><tr><td colspan=\"2\" style=\"padding:0 32px\">\
  <h2 style=\"font-size:16px;color:#8a857a\">Also in this issue</h2></td></tr>\
  <tr><td width=\"50%\" valign=\"top\" style=\"padding:0 16px 24px 32px\"><strong>Tools</strong><br>A checklist \
  for release notes people actually read.</td><td width=\"50%\" valign=\"top\" style=\"padding:0 32px 24px 16px\">\
  <strong>Interview</strong><br>A support lead on writing replies that close tickets.</td></tr>\
  <tr><td colspan=\"2\" style=\"padding:16px 32px;background:#faf8f3;color:#8a857a;font-size:12px\">You get this \
  because you subscribed at fieldnotes.example.org. <a href=\"https://fieldnotes.example.org/unsubscribe\" \
  style=\"color:#8a857a\">Unsubscribe</a></td></tr></table></td></tr></table>";
