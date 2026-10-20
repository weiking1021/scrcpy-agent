# Writing a theme

A theme is a stylesheet that is put on top of the program's own. It can change the variables the
program's stylesheet is made of (colours, fonts, the roundness of corners, shadows, a background
picture), and it may add rules of its own.

## Where a theme goes

A folder in the `themes` folder of the data folder (the settings window has a button that opens it).
The name of the folder is the id of the theme: 1 to 64 ASCII letters, digits, `_` or `-`.

```
themes/
  my-theme/
    theme.json
    theme.css
```

`theme.json`:

```json
{ "name": "My theme", "base": "dark" }
```

- `name`: what the settings window lists it as (1 to 60 characters).
- `base`: what the theme is made for, `light`, `dark` or `auto` (the system's own choice; the default).
  It decides the colour scheme of the parts the system draws (scroll bars, form controls) and which of the
  program's own colour sets is under yours.

`theme.css` is at most 256 KiB. Pictures and fonts in the folder (`png`, `jpg`, `jpeg`, `gif`, `webp`, `svg`,
`woff2`, `woff`, `ttf`, `otf`, each at most 8 MiB) are meant to be used with relative paths, `url(bg.png)`.
*(The program does not serve them to the windows yet.)*

A folder that is not used (no manifest, a manifest that is not valid, a stylesheet that is too large, an id that is
one of the program's own) is listed with its reason by the program; it is never silently skipped.

## Try it

Open `docs/theming/preview.html` in a browser. It is made of the same markup as the program's windows, with invented
data:

- `preview.html?theme=terminal` — a built-in theme (`src-tauri/src/core/theme/builtin/`)
- `preview.html?css=../../my-theme.css&base=dark` — a stylesheet of your own

## What is stable

The **variables** and the **`data-` attributes** below. Class names are not promised: they may change with the
program. Your stylesheet is not in a layer and the program's is, so yours wins whatever the selectors are.

| Variable | What |
|---|---|
| `--bg` `--surface` `--text` `--muted` `--border` | page, panels, text, quiet text, lines |
| `--accent` `--accent-text` `--accent-soft` | the colour of what is chosen, the text on it, a soft form of it |
| `--ok` `--warn` `--bad` `--info` | good, careful, wrong, neutral |
| `--open-screen-text` | the text on the green open-screen button |
| `--backdrop` | what dims the window behind a dialog |
| `--state-online` `--state-unauthorized` `--state-offline` `--state-discovered` `--state-disconnected` | the light that says the state of a device |
| `--light-size` `--light-radius` | its size and shape (`0` for a square) |
| `--radius-lg` `--radius-md` `--radius-sm` `--radius-pill` | roundness: panels and dialogs, controls, small things, pills |
| `--border-width` | the width of lines |
| `--surface-shadow` `--control-shadow` `--toast-shadow` `--popup-shadow` `--drop-shadow` | depth |
| `--font` `--font-heading` `--font-mono` `--font-size` `--line-height` | type |
| `--heading-transform` `--heading-spacing` | the small headings of the settings (`uppercase` and a little space) |
| `--bg-image` | a picture behind everything (`url(...)`, a gradient, or `none`) |
| `--sidebar-width` `--nav-height` | how wide the sidebar is open, and how tall an entry is |

`data-` attributes: `data-state` on a device card and on its light (`online`, `unauthorized`, `offline`, `discovered`,
`disconnected`), `data-base` on the root.

## One rule about the sidebar

Folded, the sidebar is as wide as what is in it: an icon with the padding and the frame an entry has. If you draw
something along the inside edge of the sidebar (an extra line, a shadow), the icons look off-centre, because the room
they are in is not the room that can be seen. Draw such things on the outside, or leave them out.

## From an add-on

`core.themes().register_theme(addon, id, name, base, css)` registers a theme (the id becomes `addon/id`); it is gone
when the add-on is removed. A stylesheet of an add-on is only the stylesheet (use `data:` for a picture, for now).
