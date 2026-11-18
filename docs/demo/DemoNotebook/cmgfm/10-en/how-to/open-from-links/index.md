---
id: 0c30c7f4bda230c55775
title: "Open a page from a link"
order: 90
created: 2026-10-18T00:00:00Z
updated: 2026-10-18T00:00:00Z
tags: []
---

## Planned: links that open Nocturne

Nocturne is planned to support links that open a specific notebook page from a browser, email,
chat, or another application. The operating system will use the `nocturne:` link type to
launch Nocturne and pass it the page to open. Depending on your operating system and browser,
you may be asked to confirm before the app opens.

This feature is **not available yet**. Nocturne does not currently register `nocturne:` with
your operating system or handle an external link by navigating to its page. Copying a
Nocturne page link does not mean it can be opened from a browser today.

## How registration is planned to work

- **Windows installer:** register Nocturne during installation and remove that registration
  when Nocturne is uninstalled.
- **Windows portable ZIP:** provide a separate **Register Nocturne links** action that you
  choose to run, plus a way to undo it. Extracting the ZIP alone will not register the link.
- **macOS:** declare the link type in the Nocturne app bundle.
- **Linux:** install a desktop application association for the link type.

Registration only tells the operating system which application can receive a link. Nocturne
must also find the matching notebook and page, handle links when it is already running, and
explain clearly when a notebook or page cannot be found. That handling is part of the planned
feature too.
