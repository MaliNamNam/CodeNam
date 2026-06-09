//! Truthful observability for *background* AX actions (issue #348).
//!
//! Background Accessibility actions (`press`, `set_value`, `perform_action`)
//! act on a UI element by reference: no cursor moves and the target app need
//! not be frontmost. That is the polite, non-disruptive path, but it has a
//! trust cost: the agent can change the user's live machine with **no signal
//! that anything happened**. This module adds a lightweight, *truthful* signal.
//!
//! The decision is based on **visibility / occlusion, not focus** (the two are
//! orthogonal on macOS: a window can be unfocused yet fully visible):
//!
//! - **Target's window is actually visible** at the element's rect (focused or
//!   not, and topmost there): draw a brief, click-through, non-activating
//!   highlight on the element's on-screen bounds (~280ms fade).
//! - **Occluded / minimized / hidden / on another Space or display**: do **not**
//!   draw a positional highlight (it would float over an unrelated window the
//!   user is actually looking at, which is misleading). Fall back to a
//!   **non-positional notice** appended to the tool output.
//!
//! Anti-goals honored (see #348): no fake "ghost cursor", no focus stealing, no
//! fullscreen overlay, the overlay is click-through + non-activating, and the
//! animation is fire-and-forget so it adds no latency to the action itself. The
//! signal is observability only; it is **not** a confirmation gate and is never
//! load-bearing for safety.

use super::osa;
use serde::Deserialize;
use std::time::Duration;

/// Env var to disable the on-screen highlight / occlusion notice. Enabled by
/// default; set to a falsey value (`0`, `off`, `false`, `no`) to turn it off.
const HIGHLIGHT_ENV: &str = "JCODE_COMPUTER_HIGHLIGHT";

/// Where the target element is, relative to what the user can actually see.
#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    /// On-screen and topmost at the element's rect (top-left global points).
    Visible { x: f64, y: f64, w: f64, h: f64 },
    /// On screen but covered by another app's window at the element's center.
    Occluded { by: String },
    /// Minimized, hidden, or on another Space/display: not currently on screen.
    Offscreen,
    /// Could not resolve the element's frame (no window, bad path, AX error).
    Unresolved,
}

/// Is the observability signal enabled? Pure helper over the raw env value so it
/// can be unit-tested without touching the process environment.
pub fn highlight_enabled_from(raw: Option<&str>) -> bool {
    match raw {
        None => true,
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no" | "none" | "disabled"
        ),
    }
}

fn highlight_enabled() -> bool {
    highlight_enabled_from(std::env::var(HIGHLIGHT_ENV).ok().as_deref())
}

/// Run the post-action observability signal for a background AX action and
/// return a one-line notice to append to the tool output (or `None` to add
/// nothing). When the target is visible it also fires the highlight flash.
///
/// This never fails the action: any error resolving visibility degrades to
/// `None` so the (already-completed) action's result is unaffected.
pub fn signal_background_action(app: &str, path: &[u32]) -> Option<String> {
    if !highlight_enabled() {
        return None;
    }
    match element_visibility(app, path) {
        Visibility::Visible { x, y, w, h } => {
            flash_rect(x, y, w, h);
            Some(format!(
                "observability: briefly highlighted the target in {app} on screen \
                 (background action, no cursor moved)."
            ))
        }
        Visibility::Occluded { by } => Some(format!(
            "observability: {app}'s window is covered by {by} right now, so no highlight was \
             drawn (it would float over {by} and mislead). The action ran in the background."
        )),
        Visibility::Offscreen => Some(format!(
            "observability: {app} is off-screen (minimized/hidden or on another Space/display), \
             so no highlight was drawn. The action ran in the background."
        )),
        Visibility::Unresolved => None,
    }
}

/// Resolve an element's on-screen frame and decide whether it is actually
/// visible, occluded, or off-screen. One JXA round-trip: read the AX
/// position/size, then hit-test the element center against the front-to-back
/// on-screen window list (`CGWindowListCopyWindowInfo`).
pub fn element_visibility(app: &str, path: &[u32]) -> Visibility {
    let script = visibility_script(app, path);
    let raw = match osa::run_jxa_timeout(&script, Duration::from_secs(4)) {
        Ok(s) => s,
        Err(_) => return Visibility::Unresolved,
    };
    parse_visibility(&raw)
}

/// Parse the JSON verdict emitted by [`visibility_script`].
fn parse_visibility(raw: &str) -> Visibility {
    #[derive(Deserialize)]
    struct Verdict {
        vis: String,
        #[serde(default)]
        by: String,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
        #[serde(default)]
        w: f64,
        #[serde(default)]
        h: f64,
    }
    let v: Verdict = match serde_json::from_str::<Verdict>(raw.trim()) {
        Ok(v) => v,
        Err(_) => return Visibility::Unresolved,
    };
    match v.vis.as_str() {
        "visible" if v.w > 0.0 && v.h > 0.0 => Visibility::Visible {
            x: v.x,
            y: v.y,
            w: v.w,
            h: v.h,
        },
        "occluded" => Visibility::Occluded { by: v.by },
        "offscreen" => Visibility::Offscreen,
        _ => Visibility::Unresolved,
    }
}

/// Build the JXA that resolves the element rect and computes the occlusion
/// verdict. `app` is JSON-quoted; `path` is a literal int array we control.
fn visibility_script(app: &str, path: &[u32]) -> String {
    let app_lit = serde_json::to_string(app).unwrap_or_else(|_| "\"\"".to_string());
    let path_lit = path
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"
ObjC.import('CoreGraphics');
ObjC.import('Foundation');
function run() {{
  var APP = {app_lit};
  var PATH = [{path_lit}];
  var rect;
  try {{
    var se = Application('System Events');
    var el = se.processes.byName(APP).windows[0];
    for (var i = 0; i < PATH.length; i++) {{ el = el.uiElements[PATH[i] - 1]; }}
    var p = el.position();
    var s = el.size();
    rect = {{ x: p[0], y: p[1], w: s[0], h: s[1] }};
  }} catch (e) {{
    return JSON.stringify({{ vis: 'unresolved' }});
  }}
  if (!rect || rect.w <= 0 || rect.h <= 0) {{
    return JSON.stringify({{ vis: 'unresolved' }});
  }}
  var cx = rect.x + rect.w / 2;
  var cy = rect.y + rect.h / 2;
  // Front-to-back on-screen windows; first one covering the center wins.
  var opts = $.kCGWindowListOptionOnScreenOnly | $.kCGWindowListExcludeDesktopElements;
  var arr = $.CGWindowListCopyWindowInfo(opts, $.kCGNullWindowID);
  var n = $.CFArrayGetCount(arr);
  var vis = 'offscreen', by = '';
  for (var j = 0; j < n; j++) {{
    var dict = ObjC.castRefToObject($.CFArrayGetValueAtIndex(arr, j));
    // Only normal app windows (layer 0) can truly occlude another app window.
    // System overlays (menubar, Notification Center's full-screen container,
    // Dock, status items) live on higher layers and are often transparent, so
    // counting them would falsely report visible windows as "occluded".
    var layer = dict.objectForKey($('kCGWindowLayer'));
    var layerN = layer ? ObjC.unwrap(layer) : 0;
    if (layerN !== 0) {{ continue; }}
    var owner = dict.objectForKey($('kCGWindowOwnerName'));
    var ownerS = owner ? ObjC.unwrap(owner) : '';
    var b = ObjC.deepUnwrap(dict.objectForKey($('kCGWindowBounds'))) || {{}};
    if (cx >= b.X && cx <= b.X + b.Width && cy >= b.Y && cy <= b.Y + b.Height) {{
      if (ownerS === APP) {{ vis = 'visible'; }} else {{ vis = 'occluded'; by = ownerS; }}
      break;
    }}
  }}
  return JSON.stringify({{ vis: vis, by: by, x: rect.x, y: rect.y, w: rect.w, h: rect.h }});
}}
"#
    )
}

/// Fire-and-forget a brief click-through, non-activating highlight on a global
/// top-left rect. Runs in a detached thread so the action returns immediately;
/// the thread reaps the short-lived `osascript` child (no zombies, no blocking).
pub fn flash_rect(x: f64, y: f64, w: f64, h: f64) {
    let script = flash_script(x, y, w, h);
    std::thread::spawn(move || {
        // Bound it so a wedged WindowServer can never leak a stuck helper.
        let _ = osa::run_command_timed(
            "/usr/bin/osascript",
            &["-l", "JavaScript", "-e", &script],
            Duration::from_secs(5),
        );
    });
}

/// Build the JXA overlay script. Coordinates are global top-left points; the
/// script converts to Cocoa's bottom-left global space using the height of the
/// primary screen (the one anchored at origin 0,0).
fn flash_script(x: f64, y: f64, w: f64, h: f64) -> String {
    format!(
        r#"
ObjC.import('Cocoa');
function run() {{
  var X = {x}, Y = {y}, W = {w}, H = {h};
  var screens = $.NSScreen.screens;
  var primaryH = 0.0;
  for (var i = 0; i < screens.count; i++) {{
    var f = screens.objectAtIndex(i).frame;
    if (f.origin.x === 0 && f.origin.y === 0) {{ primaryH = f.size.height; break; }}
  }}
  if (primaryH === 0) {{ primaryH = $.NSScreen.mainScreen.frame.size.height; }}
  var ay = primaryH - Y - H; // flip top-left -> Cocoa bottom-left
  var rect = $.NSMakeRect(X, ay, W, H);
  var win = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(rect, 0, 2, false);
  win.setOpaque(false);
  win.setBackgroundColor($.NSColor.colorWithSRGBRedGreenBlueAlpha(0.20, 0.85, 0.50, 0.16));
  win.setLevel(25);                 // NSStatusWindowLevel: above normal windows
  win.setIgnoresMouseEvents(true);  // click-through
  win.setHasShadow(false);
  win.setCollectionBehavior((1 << 0) | (1 << 4)); // canJoinAllSpaces | stationary
  var view = win.contentView;
  view.setWantsLayer(true);
  var layer = view.layer;
  layer.setBorderWidth(3.0);
  layer.setCornerRadius(6.0);
  layer.setBorderColor($.NSColor.colorWithSRGBRedGreenBlueAlpha(0.20, 0.85, 0.50, 0.95).CGColor);
  win.orderFrontRegardless;         // show without activating
  var steps = 7, dt = 0.04;         // ~280ms fade-out
  for (var s = 0; s < steps; s++) {{
    $.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(dt));
    win.setAlphaValue(1.0 - (s + 1) / steps);
  }}
  win.close;
  return 'ok';
}}
"#
    )
}

/// Test seam: expose the generated scripts so unit tests can assert structure
/// without a GUI.
#[cfg(test)]
pub(super) fn visibility_script_for_test(app: &str, path: &[u32]) -> String {
    visibility_script(app, path)
}

#[cfg(test)]
pub(super) fn flash_script_for_test(x: f64, y: f64, w: f64, h: f64) -> String {
    flash_script(x, y, w, h)
}

/// Test seam: parse a raw verdict string.
#[cfg(test)]
pub(super) fn parse_visibility_for_test(raw: &str) -> Visibility {
    parse_visibility(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_enabled_defaults_on() {
        assert!(highlight_enabled_from(None));
        assert!(highlight_enabled_from(Some("1")));
        assert!(highlight_enabled_from(Some("yes")));
        assert!(highlight_enabled_from(Some("anything")));
    }

    #[test]
    fn highlight_disabled_by_falsey_values() {
        for v in [
            "0", "off", "OFF", "false", "no", "none", "disabled", " false ",
        ] {
            assert!(!highlight_enabled_from(Some(v)), "{v} should disable");
        }
    }

    #[test]
    fn parses_visible_verdict() {
        let raw = r#"{"vis":"visible","by":"","x":100,"y":200,"w":300,"h":40}"#;
        assert_eq!(
            parse_visibility_for_test(raw),
            Visibility::Visible {
                x: 100.0,
                y: 200.0,
                w: 300.0,
                h: 40.0,
            }
        );
    }

    #[test]
    fn visible_with_zero_size_is_unresolved() {
        // A degenerate rect must not produce a positional highlight.
        let raw = r#"{"vis":"visible","x":10,"y":10,"w":0,"h":0}"#;
        assert_eq!(parse_visibility_for_test(raw), Visibility::Unresolved);
    }

    #[test]
    fn parses_occluded_verdict_keeps_coverer() {
        let raw = r#"{"vis":"occluded","by":"Safari"}"#;
        assert_eq!(
            parse_visibility_for_test(raw),
            Visibility::Occluded {
                by: "Safari".into()
            }
        );
    }

    #[test]
    fn parses_offscreen_and_unresolved() {
        assert_eq!(
            parse_visibility_for_test(r#"{"vis":"offscreen"}"#),
            Visibility::Offscreen
        );
        assert_eq!(
            parse_visibility_for_test(r#"{"vis":"unresolved"}"#),
            Visibility::Unresolved
        );
        assert_eq!(
            parse_visibility_for_test("not json"),
            Visibility::Unresolved
        );
    }

    #[test]
    fn visibility_script_quotes_app_and_inlines_path() {
        // App name with a quote must be JSON-escaped, not break the script.
        let script = visibility_script_for_test("My \"App\"", &[2, 5, 1]);
        assert!(
            script.contains(r#"var APP = "My \"App\"";"#),
            "got: {script}"
        );
        assert!(script.contains("var PATH = [2,5,1];"));
        assert!(script.contains("CGWindowListCopyWindowInfo"));
        // Only normal app windows (layer 0) count as occluders, so transparent
        // system overlays (Notification Center, menubar) don't cause false
        // "occluded" verdicts.
        assert!(script.contains("kCGWindowLayer"));
        assert!(script.contains("layerN !== 0"));
        // Empty path is valid (the front window itself).
        let empty = visibility_script_for_test("Finder", &[]);
        assert!(empty.contains("var PATH = [];"));
    }

    #[test]
    fn flash_script_has_clickthrough_and_nonactivating_markers() {
        let script = flash_script_for_test(600.0, 400.0, 200.0, 120.0);
        // Click-through + show-without-activate + above-normal level are the
        // load-bearing guarantees from the issue's anti-goals.
        assert!(script.contains("setIgnoresMouseEvents(true)"));
        assert!(script.contains("orderFrontRegardless"));
        assert!(script.contains("setLevel(25)"));
        // Coordinates are interpolated.
        assert!(script.contains("var X = 600"));
        assert!(script.contains("H = 120"));
        // Coordinate flip from top-left to Cocoa bottom-left is present.
        assert!(script.contains("primaryH - Y - H"));
    }
}
