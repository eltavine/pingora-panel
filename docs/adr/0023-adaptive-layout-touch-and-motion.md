# 0023: Adaptive layout, touch and motion

Status: accepted.

## Context

The console is used at a desk and on phones and tablets during incidents. A
layout that only shrinks the desktop leaves people on phones with a drawer
behind a menu button for every move, controls too small for a finger, and
content under notches and home indicators. Motion that helps some people
makes others dizzy. The console also has to load quickly on a phone
connection.

Two widely followed guides agree on the essentials:
[Material 3](https://m3.material.io/foundations/layout/breakpoints/overview)
sizes layouts by window size classes and picks navigation by them, and
[Apple's Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines/accessibility)
set the minimum touch target, keep content inside safe areas and ask apps to
honor Reduce Motion. The visual system of
[ADR 0005](0005-web-console-stack.md) stays: monochrome, icon plus text,
WCAG 2.2 AA.

## Decision

**Window size classes.** The shell follows Material 3's width classes:
compact below 600 px, medium from 600 px, expanded from 840 px, large from
1200 px and extra-large from 1600 px. Content grids keep their own
breakpoints.

**Navigation by window size.**

- Compact windows show a navigation bar at the bottom with up to four of the
  most used destinations the account may open, each an icon with a label,
  and a *More* entry that opens every destination in a modal drawer. This is
  Material's navigation bar and Apple's tab bar with its *More* tab.
- Medium windows show a navigation rail: the sidebar collapsed to icons.
- Expanded and larger windows show the full navigation drawer, which people
  can collapse to the rail.

A feature marks the destinations it offers to the navigation bar and their
order; the shell picks the first four the account can open.

**Touch.** On coarse pointers every control is at least 44 by 44 px, Apple's
default control size and within Material's 48 dp touch target once spacing
is counted: buttons, inputs, selects and navigation entries grow to 44 px,
and smaller controls keep their look but extend their hit area. Fine pointers
keep the denser desktop sizes, which meet Apple's 28 pt guidance for
pointers.

**Safe areas.** The page extends under display cutouts and draws its own
insets: the header, the content and the navigation bar are padded by the
`safe-area-inset-*` environment values.

**Reflow.** Every page fits a window 320 px wide without scrolling sideways
([WCAG 2.2 SC 1.4.10](https://www.w3.org/TR/WCAG22/#reflow)). Wide tables
scroll inside their own container, tab lists scroll sideways, page actions
wrap below the title, and layouts that sit beside the navigation size
themselves by their container rather than by the window.

**Motion.** Transitions use Material's standard easing,
`cubic-bezier(0.2, 0, 0, 1)`, and short durations. When people ask for
reduced motion, animations and transitions are cut to a single frame and
spinners stop, so a state still changes but nothing moves.

**Performance.** The first page loads at most 200 KiB of compressed scripts
and styles: features load with their routes, the configuration editor loads
only where it is used, and only the messages of the chosen language load at
startup; the other language loads when someone switches to it.

## Consequences

- Phones get one-tap access to the pages used most, with every other page
  one more tap away, and tablets and desktops keep persistent navigation.
- Adding a page to the navigation bar is a choice in its feature module, not
  a change to the shell.
- Control sizes depend on the pointer, so tests check them on touch devices
  as well as on the desktop.
- Tests render every page with the sample data of the preview mode at 320,
  700 and 1024 px and fail when one scrolls sideways.
