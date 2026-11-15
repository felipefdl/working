# Demo photos

The pictures demo mode (`WORKING_DEMO=1`) shows as image attachments and link-preview images. `demo/image.rs` embeds each file with `include_bytes!` and declares its size; `Core::fetch` serves it from memory at `demo:image/<file>`.

None of them is a photograph of anything real. Each was generated on 2026-11-14 with Grok's image model (Grok Imagine) from the prompt below, then stripped of all metadata (EXIF and C2PA) and re-encoded as a baseline JPEG at its native size, quality 80. No people, logos, brand names, or readable text appear in them.

| File | Size | Bytes | Shown in |
| --- | --- | --- | --- |
| `ramen-bowl.jpg` | 1152 × 864 (4:3) | 182,748 | Riley's DM: "Yes! This is what I'm getting" (`ramen.jpg`) |
| `launch-hero-light.jpg` | 1248 × 832 (3:2) | 101,731 | The launch space: "And the hero images, light and dark:" (`hero-light.jpg`), and the preview of the staging landing page link |
| `launch-hero-dark.jpg` | 1248 × 832 (3:2) | 65,904 | The launch space: "And the hero images, light and dark:" (`hero-dark.jpg`) |
| `coffee-beans.jpg` | 1152 × 864 (4:3) | 136,509 | Coffee chat: "New beans in the kitchen, a washed Ethiopian. Very bright." (`coffee-beans.jpg`) |

A replacement keeps the same rules: generated, never downloaded; a 4:3 or 3:2 landscape JPEG, metadata stripped, roughly 80 to 200 KB, the set under about 1 MB; its size updated in `demo/image.rs` and its prompt and date recorded here.

## Prompts

### `ramen-bowl.jpg`

> A realistic phone photo taken from above at a slight angle of a steaming bowl of tonkotsu ramen on a dark wooden restaurant table: chashu pork slices, a halved soft-boiled egg with jammy yolk, nori sheets, scallions and bamboo shoots. Chopsticks rest on the bowl's rim, a small glass of water and a napkin at the edge of the frame. Warm natural window light, shallow depth of field, no people, no text, no logos.

### `launch-hero-light.jpg`

> A realistic photograph of a bright, minimal desk by a large window in morning daylight: an open silver laptop with no visible logo, its screen showing a clean light-themed analytics dashboard of soft blue line charts, bar charts and rounded cards with no readable text. Charts have no axis labels, no numbers, no letters, and no words; only unlabeled geometric chart shapes on white cards. A small potted plant and a white ceramic mug sit beside it on a pale oak desk; white walls, soft shadows, airy and calm. No people, no logos, no brand names.

### `launch-hero-dark.jpg`

> A realistic photograph of the same kind of minimal desk at night, lit only by a warm desk lamp and the glow of an open laptop with no visible logo. The laptop screen shows a dark-themed analytics dashboard with glowing teal and violet line charts and bar charts on a near-black background, no readable text. A potted plant and a mug in soft shadow, city lights blurred through the window behind. No people, no logos, no brand names.

### `coffee-beans.jpg`

> A realistic phone photo on an office kitchen counter: an open kraft paper coffee bag spilling light-to-medium roasted coffee beans onto a light wood surface, next to a hand burr grinder and a glass pour-over carafe. Bright natural morning light from a window, shallow depth of field. The bag has no label text and no logo; no people.
