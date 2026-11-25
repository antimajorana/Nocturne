# Commands

Generated from the command registry. Do not edit by hand: run
`UPDATE_DOCS=1 cargo test -p nocturne-engine commands_doc` to refresh it.
Every front end (window, `nocturne-cli run`, the control channel) uses these ids.

## `notebook.create`

New notebook (changes the tree).

Parameters:

- name (required): the notebook's title
- format (optional): html or cmgfm; default: the global setting
- empty (optional): true to leave the notebook empty instead of creating the first section and page

## `node.create`

New section group, section or page (changes the tree).

Parameters:

- parent (required): path of the notebook, group, section or page to add to
- kind (required): section-group, section or page
- title (required for section-group and section; optional for page, which is created untitled if left out)
- format (optional): html or cmgfm. For a page, its format; for a group or section, the default for pages below

## `node.demote`

Make a page a subpage of its preceding sibling (changes the tree).

Parameters:

- path (required): the page to make a subpage

## `node.promote`

Promote a subpage to its parent's level (changes the tree).

Parameters:

- path (required): the subpage to promote

## `node.move`

Move a page, section or section group into another notebook, section group, section or page (changes the tree).

Parameters:

- path (required): the page, section or section group to move
- to (required): where it goes; it is put last there

## `node.copy`

Copy a page, section or section group into another notebook, section group, section or page (changes the tree).

Parameters:

- path (required): the page, section or section group to copy
- to (required): where the copy goes; it is put last there, and every node in it gets a new id

## `page.templates`

List the page templates, built in and saved: one line each, with the id, the category and the name, separated by tabs.

No parameters.

## `page.from_template`

A new page started from a template (changes the tree).

Parameters:

- parent (required): the section (or page, for a subpage) the new page goes in, last
- template (required): the template's id, as page.templates lists it (builtin:meeting-notes, saved:lab-report)

## `page.save_template`

Keep a page as a template.

Parameters:

- path (required): the page; its words, boxes, ink, files, paper and colour are kept, not its subpages
- name (required): the template's name

## `page.remove_template`

Take away a page saved as a template.

Parameters:

- template (required): the saved template's id (saved:...). Built-in templates cannot be removed

## `node.move_up`

Move a page up among its siblings (changes the tree).

Parameters:

- path (required): the page to move up

## `node.renumber`

Renumber the pages of a section or page ten apart, keeping their order (changes the tree).

Parameters:

- path (required): the section or page whose child pages are renumbered

## `node.move_down`

Move a page down among its siblings (changes the tree).

Parameters:

- path (required): the page to move down

## `node.color`

Colour a notebook, section group or section (changes the tree).

Parameters:

- path (required): the notebook, group or section
- color (required): purple, blue, teal, green, yellow, orange, red, magenta, gray, or none for the theme's own colour

## `node.format`

Set the default page format for a notebook, section group or section (changes the tree).

Parameters:

- path (required): the notebook, group or section
- format (required): html, cmgfm, or none to inherit the parent or global format

## `node.delete`

Move a page, section or group into its notebook's recycle bin (changes the tree).

Parameters:

- path (required): the page, section or group to recycle

## `recycle.list`

List recycled items in a notebook.

Parameters:

- notebook (required): the notebook whose recycle bin to list

## `recycle.restore`

Restore an item from a notebook's recycle bin (changes the tree).

Parameters:

- notebook (required)
- id (required): an id returned by recycle.list

## `recycle.purge`

Permanently delete an item from a notebook's recycle bin (changes the tree).

Parameters:

- notebook (required)
- id (required): an id returned by recycle.list

## `node.title`

Rename the title of a node (changes the tree).

Parameters:

- path (required)
- title (required)

## `page.load`

Open a page.

Parameters:

- path (required): the page

## `page.save`

Save a page.

Parameters:

- path (required): the page
- body (required): the editable part (inside of the HTML body, or the Markdown after the front matter)

## `page.versions`

List saved page versions.

Parameters:

- path (required): the page

## `page.version.restore`

Restore a saved page version.

Parameters:

- path (required): the page
- id (required): the saved version id from page.versions

## `page.direction`

Which way a page reads.

Parameters:

- path (required): the page
- dir (optional): rtl, ltr, or auto to let every line decide; without it the current setting is reported

## `page.containers`

How a page's blocks are arranged in boxes.

Parameters:

- path (required): the page
- boxes (optional): x,y,width,blocks per container, separated by semicolons. The counts must add up to the page's blocks. They are written into the page's text as elements around the blocks; empty puts the page back in one column. Without it, the current arrangement is reported

## `page.canvas_layout`

Choose the canvas layout for a page.

Parameters:

- path (required): the page
- layout (optional): infinite, portrait, or landscape. Omit to read the current layout

## `page.paper`

The paper a page is drawn on, the colour behind it and the colour of its lines.

Parameters:

- path (required): the page
- paper (optional): none, narrow, college, standard, wide, grid-small, grid-medium, grid-large, grid-huge, craft-lines, craft-grid
- color (optional): purple, blue, teal, green, yellow, orange, red, magenta, gray, or none
- rules (optional): the colour of the rule and grid lines, one of the same colours, or none for the theme's own grey. Without paper or color, the page's own are reported, followed by the lines' colour when it has one

## `page.columns`

How wide the columns of a page's tables are, which tables have their borders hidden, and how their cells are aligned and shaded.

Parameters:

- path (required): the page
- widths (optional): block:w,w,w per table, separated by semicolons, with a ! after the widths (or on its own, as in 3:!) for a table whose borders are hidden, and block@row.column=a,row.column=a for cells aligned l(eft), c(enter) or r(ight) and shaded with ~ and a colour (purple, blue, teal, green, yellow, orange, red, magenta or gray), as in 2@1.0=c~yellow, with the heading as row 0; empty puts every table back to the usual width, borders, alignment and shading. Without it, the current layout is reported

## `page.justify`

Which paragraphs of a page are stretched to fill the column.

Parameters:

- path (required): the page
- blocks (optional): the justified paragraphs' positions among the page's blocks, separated by commas, each with an optional strength after a colon (low, medium or high; a number on its own is medium), as in '4, 11:high'; empty draws every paragraph ragged. Without it, the page's own are reported

## `page.attach`

Attach a file to a page.

Parameters:

- path (required): the page
- source (required): a file to copy into the page's assets folder

## `page.detach`

Take a file out of a page's folder.

Parameters:

- path (required): the page
- name (required): the file in the page's assets folder, as page.load reports it. The file is deleted; a line that linked to it is left alone

## `settings.default_format.get`

Show the global default format.

No parameters.

## `settings.default_format.set`

Set the global default format.

Parameters:

- format (required): html or cmgfm

## `search.query`

Search all pages.

Parameters:

- q (required): text to find
- limit (optional): at most this many hits (default 40, at most 500)

## `tags.find`

Find tagged notes.

Parameters:

- scope (optional): a page, section, section group or notebook to look in, relative to the notebooks folder; every open notebook when left out
- page (optional): true to look in the scope page alone, without its subpages
- tag (optional): only this tag, by its id (todo, important, question and the rest)
- unchecked (optional): true for only the To Do items not yet ticked
- limit (optional): at most this many lines (default 500, at most 2000)

## `notebook.open`

Open a notebook or initialize an empty folder (changes the tree).

Parameters:

- path (required): a notebook folder anywhere on this machine, or an empty folder to initialize as a notebook named after the folder. The notebooks already open stay open, and this one is opened again next time

## `open.page`

Find the page to show for a notebook, section group, section or page (changes the tree).

Parameters:

- path (required): relative to the notebooks folder, or a full path. A page is itself; anything else gives its first page. A notebook kept somewhere else is opened first, and one that was put away is opened again

## `notebook.close`

Put away a notebook without deleting it (changes the tree).

Parameters:

- path (required): the notebook's folder. Nothing is deleted

## `notebook.check`

Check a notebook against the folder rules.

Parameters:

- path (required): the notebook

## `tree.read`

List the notebooks, sections and pages.

Parameters:

- path (optional): start at this node instead of listing every notebook

## `tree.refresh`

Reload the tree from disk.

No parameters.

## `view.tablet_tools`

Minimize or expand the Tablet Studio tools.

Parameters:

- minimized (required): true or false

## `ink.toggle_eraser`

Toggle eraser.

Parameters:

- mode (optional): toggle, eraser, or pen; default: toggle

## `ink.finger_drawing`

Toggle finger drawing.

Parameters:

- enabled (optional): true, false, or toggle; default: toggle

## `ink.eraser_mode`

Choose what the eraser takes.

Parameters:

- mode (optional): object (the whole stroke), pixel (only what is under the eraser), or toggle; default: toggle

## `ink.eraser_width`

Set the eraser's size.

Parameters:

- width: xs, s, m, l, xl, xxl, xxxl, or points across from 2 to 80

## `ink.hold_to_straighten`

Snap a hand-drawn line or square when the pen is held still.

Parameters:

- enabled (optional): true, false, or toggle; default: toggle

## `ink.math_ocr_toggle`

Turn handwritten equation recognition on or off.

Parameters:

- enabled (optional): true, false, or toggle; default: toggle

## `ink.handwriting_ocr_toggle`

Turn general handwriting-to-text recognition on or off.

Parameters:

- enabled (optional): true, false, or toggle; default: toggle

## `ink.math_ocr_configure`

Choose the service that reads handwritten equations, and give it its key.

Parameters:

- provider (optional): apple_vision, local_model, anthropic, gemini, openai, mathpix, or custom
- endpoint (optional): the service's address, https only unless it is on this machine; default for the service's own
- model (optional): the model to ask; default for the service's own
- key (optional): the service's key, kept outside the settings file and never shown
- app_id (optional): the app id Mathpix needs beside its key
- clear_key (optional): true forgets the key
- without parameters, reports the settings

## `ink.math_ocr_convert`

Read a page's handwritten equation or text (sends a picture of the ink to the chosen service).

Parameters:

- path (required): the page
- strokes (optional): which strokes, by position, comma separated; default: all of them
- mode (optional): math (default) or handwriting

## `ink.math_ocr_remove_model`

Remove the downloaded on-device math recognition model files.

No parameters.

## `ink.configure_tools`

Choose the tools on the Tablet Studio tool bar.

Parameters:

- tools (optional): the tools in order, comma separated, a minus before one to hide it (write, pens, eraser, lasso, select, shapes, picture, math_ocr, history); without it, reports the list

## `ui.toolbar_dock`

Move the Tablet Studio tool bar to an edge.

Parameters:

- edge: bottom, top, left, or right

## `ui.library_view`

Choose how Tablet Studio shows the notebooks and pages.

Parameters:

- view (optional): studio_grid, classic_tree, or toggle; without it, reports the current view

## `ui.layout_mode`

Choose whether Nocturne uses Tablet Studio, Classic Ribbon, or automatic layout.

Parameters:

- mode (optional): auto, tablet_studio (or tablet), or classic_ribbon (or classic); without it, reports the current mode

## `sync.status`

Show Infomaniak kDrive sync settings and last sync summary.

No parameters.

## `sync.configure`

Configure Infomaniak kDrive sync settings and Personal API Token (changes the tree).

Parameters:

- endpoint (optional): kDrive API base URL (default https://api.infomaniak.com)
- drive_id (optional): numeric kDrive ID, or auto to detect automatically
- remote_path (optional): folder path inside kDrive (default The Oracle/Nocturne)
- auto_sync (optional): true or false
- markdown_default (optional): true or false; when true, sets global default_format to cmgfm
- propagate_deletes (optional): true or false; default false
- token (optional): Personal API Token with drive scope, stored only in the secret store
- clear_token (optional): true to remove the stored token
- convert_notebooks_to_markdown (optional): true to set _node.toml format = cmgfm on existing user notebooks

## `sync.now`

Run a two-way kDrive sync for the notebooks folder (changes the tree).

No parameters.

