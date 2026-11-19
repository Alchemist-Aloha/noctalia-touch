//! Keyboard layouts and Linux input for the floating keyboard.

use std::borrow::Cow;
use std::cell::RefCell;
use std::io;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, Device, FFEffect, FFEffectCode, FFEffectData, FFEffectKind, FFReplay,
    FFTrigger, InputId, KeyCode, KeyEvent,
};
use gpui_kit::SharedString;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Char(char),
    Key(&'static str),
    Space,
    Shift,
    Ctrl,
    Alt,
    Super,
    Page(Page),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Abc,
    Sym,
    More,
}

/// Labels are shared strings so rendering a frame clones a pointer, not text.
#[derive(Clone, Debug)]
pub struct Key {
    pub label: SharedString,
    /// Label shown while Shift or Caps Lock is active.
    pub upper: SharedString,
    pub action: Action,
    pub width: f32,
    pub hint: Option<(SharedString, Action)>,
}

fn key(label: &'static str, action: Action, width: f32) -> Key {
    let label = SharedString::new_static(label);
    Key {
        upper: label.clone(),
        label,
        action,
        width,
        hint: None,
    }
}

fn char_key(ch: char) -> Key {
    let hint = match ch {
        ',' => Some(("⇥".into(), Action::Key("Tab"))),
        '.' => Some(("esc".into(), Action::Key("Escape"))),
        _ => [
            ("qwertyuiop", "1234567890"),
            ("asdfghjkl", "@#$_&-+()"),
            ("zxcvbnm", "*\"':;!?"),
            ("1234567890", "¹²³¼½¾⅛°±≠"),
        ]
        .iter()
        .find_map(|(base, hints)| {
            let alt = hints.chars().nth(base.find(ch)?)?;
            Some((alt.to_string().into(), Action::Char(alt)))
        }),
    };
    let label = SharedString::from(ch.to_string());
    let upper = if ch.is_ascii_lowercase() {
        ch.to_ascii_uppercase().to_string().into()
    } else {
        label.clone()
    };
    Key {
        label,
        upper,
        action: Action::Char(ch),
        width: 1.0,
        hint,
    }
}

fn chars(text: &str) -> Vec<Key> {
    text.chars().map(char_key).collect()
}

pub fn rows(page: Page) -> Vec<Vec<Key>> {
    use Action::{Alt, Ctrl, Key as Named, Page as Switch, Shift, Space, Super};
    let strip = vec![
        key("Esc", Named("Escape"), 1.0),
        key("⇥", Named("Tab"), 1.25),
        key("Ctrl", Ctrl, 1.25),
        key("Alt", Alt, 1.25),
        key("❖", Super, 1.25),
        key("←", Named("Left"), 1.0),
        key("↓", Named("Down"), 1.0),
        key("↑", Named("Up"), 1.0),
        key("→", Named("Right"), 1.0),
    ];
    let (top, middle, mut lower, next, label) = match page {
        Page::Abc => (
            "qwertyuiop",
            "asdfghjkl",
            [vec![key("⇧", Shift, 1.5)], chars("zxcvbnm")].concat(),
            Page::Sym,
            "?123",
        ),
        Page::Sym => (
            "1234567890",
            "@#$_&-+()/",
            [
                vec![key("=\\<", Switch(Page::More), 1.5)],
                chars("*\"':;!?"),
            ]
            .concat(),
            Page::Abc,
            "abc",
        ),
        Page::More => (
            "~`|\\{}[]<>",
            "^%=€£¥°•§¶",
            vec![
                key("?123", Switch(Page::Sym), 1.5),
                key("Home", Named("Home"), 1.25),
                key("End", Named("End"), 1.25),
                key("PgUp", Named("Prior"), 1.25),
                key("PgDn", Named("Next"), 1.25),
                key("Del", Named("Delete"), 1.25),
            ],
            Page::Abc,
            "abc",
        ),
    };
    lower.push(key("⌫", Named("BackSpace"), 1.5));
    vec![
        strip,
        chars(top),
        chars(middle),
        lower,
        vec![
            key(label, Switch(next), 1.5),
            char_key(','),
            key("space", Space, 5.0),
            char_key('.'),
            key("⏎", Named("Return"), 1.5),
        ],
    ]
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    /// Physical Shift; the UI can send an uppercase `Char` without setting this.
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    fn active(self) -> impl DoubleEndedIterator<Item = (KeyCode, &'static str)> {
        [
            (self.shift, KeyCode::KEY_LEFTSHIFT, "shift"),
            (self.ctrl, KeyCode::KEY_LEFTCTRL, "ctrl"),
            (self.alt, KeyCode::KEY_LEFTALT, "alt"),
            (self.logo, KeyCode::KEY_LEFTMETA, "logo"),
        ]
        .into_iter()
        .filter_map(|(on, code, name)| on.then_some((code, name)))
    }
}

const NAMED_KEYS: &[(&str, KeyCode)] = &[
    ("Escape", KeyCode::KEY_ESC),
    ("BackSpace", KeyCode::KEY_BACKSPACE),
    ("Tab", KeyCode::KEY_TAB),
    ("Return", KeyCode::KEY_ENTER),
    ("Home", KeyCode::KEY_HOME),
    ("Up", KeyCode::KEY_UP),
    ("Prior", KeyCode::KEY_PAGEUP),
    ("Page_Up", KeyCode::KEY_PAGEUP),
    ("Left", KeyCode::KEY_LEFT),
    ("Right", KeyCode::KEY_RIGHT),
    ("End", KeyCode::KEY_END),
    ("Down", KeyCode::KEY_DOWN),
    ("Next", KeyCode::KEY_PAGEDOWN),
    ("Page_Down", KeyCode::KEY_PAGEDOWN),
    ("Delete", KeyCode::KEY_DELETE),
];

// X keysyms for printable ASCII other than letters and digits.
const ASCII_PUNCT: &[(char, &str)] = &[
    (' ', "space"),
    ('!', "exclam"),
    ('"', "quotedbl"),
    ('#', "numbersign"),
    ('$', "dollar"),
    ('%', "percent"),
    ('&', "ampersand"),
    ('\'', "apostrophe"),
    ('(', "parenleft"),
    (')', "parenright"),
    ('*', "asterisk"),
    ('+', "plus"),
    (',', "comma"),
    ('-', "minus"),
    ('.', "period"),
    ('/', "slash"),
    (':', "colon"),
    (';', "semicolon"),
    ('<', "less"),
    ('=', "equal"),
    ('>', "greater"),
    ('?', "question"),
    ('@', "at"),
    ('[', "bracketleft"),
    ('\\', "backslash"),
    (']', "bracketright"),
    ('^', "asciicircum"),
    ('_', "underscore"),
    ('`', "grave"),
    ('{', "braceleft"),
    ('|', "bar"),
    ('}', "braceright"),
    ('~', "asciitilde"),
];

fn us_key(ch: char) -> Option<(KeyCode, bool)> {
    // Linux key codes are contiguous within each of these physical rows.
    [
        ("1234567890-=", "!@#$%^&*()_+", KeyCode::KEY_1),
        ("qwertyuiop[]", "QWERTYUIOP{}", KeyCode::KEY_Q),
        ("asdfghjkl;'`", "ASDFGHJKL:\"~", KeyCode::KEY_A),
        ("\\zxcvbnm,./", "|ZXCVBNM<>?", KeyCode::KEY_BACKSLASH),
        (" ", "", KeyCode::KEY_SPACE),
    ]
    .into_iter()
    .find_map(|(plain, shifted, first)| {
        plain
            .find(ch)
            .map(|n| (KeyCode(first.0 + n as u16), false))
            .or_else(|| {
                shifted
                    .find(ch)
                    .map(|n| (KeyCode(first.0 + n as u16), true))
            })
    })
}

fn lookup(action: Action) -> Option<(KeyCode, bool)> {
    match action {
        Action::Char(ch) => us_key(ch),
        Action::Space => us_key(' '),
        Action::Key(name) => {
            if let Some((_, code)) = NAMED_KEYS.iter().find(|(sym, _)| *sym == name) {
                return Some((*code, false));
            }
            let ch = if name.len() == 1 {
                name.chars().next()?
            } else {
                match name {
                    "quoteright" => '\'',
                    "quoteleft" => '`',
                    _ => ASCII_PUNCT.iter().find(|(_, sym)| *sym == name)?.0,
                }
            };
            us_key(ch)
        }
        _ => None,
    }
}

fn wtype_args(action: Action, mods: Modifiers) -> Option<Vec<Cow<'static, str>>> {
    let sym = match action {
        Action::Char(ch) if ch.is_ascii_alphanumeric() => Cow::Owned(ch.to_string()),
        Action::Char(ch) => match ASCII_PUNCT.iter().find(|(c, _)| *c == ch) {
            Some((_, sym)) => Cow::Borrowed(*sym),
            None if !ch.is_control() => Cow::Owned(format!("U{:04X}", u32::from(ch))),
            None => return None,
        },
        Action::Key(sym) => Cow::Borrowed(sym),
        Action::Space => Cow::Borrowed("space"),
        _ => return None,
    };
    let mut args = Vec::with_capacity(3 + 4 * 4);
    for (_, name) in mods.active() {
        args.extend([Cow::Borrowed("-M"), Cow::Borrowed(name)]);
    }
    args.extend([Cow::Borrowed("-k"), sym]);
    for (_, name) in mods.active().rev() {
        args.extend([Cow::Borrowed("-m"), Cow::Borrowed(name)]);
    }
    Some(args)
}

fn wtype(args: Vec<Cow<'static, str>>, queue: &Receiver<(Action, Modifiers)>) -> io::Result<()> {
    let mut child = Command::new("wtype")
        .args(args.iter().map(AsRef::<str>::as_ref))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(io::Error::other(format!("wtype exited with {status}")));
            }
            Ok(None) if !queue.is_closed() && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(8));
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result.err().unwrap_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "wtype timed out or keyboard closed",
                    )
                }));
            }
        }
    }
}

fn virtual_keyboard() -> io::Result<VirtualDevice> {
    let modifiers = Modifiers {
        shift: true,
        ctrl: true,
        alt: true,
        logo: true,
    };
    let keys: AttributeSet<_> = (' '..='~')
        .filter_map(|ch| us_key(ch).map(|(code, _)| code))
        .chain(NAMED_KEYS.iter().map(|&(_, code)| code))
        .chain(modifiers.active().map(|(code, _)| code))
        .collect();
    VirtualDevice::builder()?
        .name("floatkb virtual keyboard")
        .input_id(InputId::new(BusType::BUS_VIRTUAL, 1, 1, 1))
        .with_keys(&keys)?
        .build()
}

fn tap(
    code: KeyCode,
    mut mods: Modifiers,
    shifted: bool,
    mut emit: impl FnMut(KeyCode, i32) -> io::Result<()>,
) -> io::Result<()> {
    mods.shift |= shifted;
    for (modifier, _) in mods.active() {
        emit(modifier, 1)?;
    }
    emit(code, 1)?;
    thread::sleep(Duration::from_millis(4));
    emit(code, 0)?;
    for (modifier, _) in mods.active().rev() {
        emit(modifier, 0)?;
    }
    thread::sleep(Duration::from_millis(4));
    Ok(())
}

fn typing_worker(queue: Receiver<(Action, Modifiers)>) {
    let mut device = virtual_keyboard().ok();
    if device.is_some() {
        // Give the compositor a chance to pick up the new input device.
        thread::sleep(Duration::from_millis(150));
    }
    let mut warned = false;
    while let Ok((action, mods)) = queue.recv_blocking() {
        if queue.is_closed() {
            break;
        }
        if let (Some(device_ref), Some((code, shifted))) = (device.as_mut(), lookup(action)) {
            if tap(code, mods, shifted, |code, value| {
                device_ref.emit(&[*KeyEvent::new(code, value)])
            })
            .is_ok()
            {
                continue;
            }
            // Unplug the failed device to clear any partially emitted chord.
            device = None;
        }
        let Some(args) = wtype_args(action, mods) else {
            continue;
        };
        match wtype(args, &queue) {
            Err(err) if !queue.is_closed() && !warned => {
                eprintln!("floatkb: cannot send keystroke via wtype: {err}");
                warned = true;
            }
            _ => {}
        }
    }
}

fn haptic_effect() -> Option<FFEffect> {
    // Only drive the system haptic motor, not arbitrary rumble-capable gamepads.
    let mut entries = std::fs::read_dir("/sys/class/input")
        .ok()?
        .flatten()
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    entries.sort_unstable();
    for name in entries {
        if !name.as_encoded_bytes().starts_with(b"event") {
            continue;
        }
        let sys_name = std::path::Path::new("/sys/class/input")
            .join(&name)
            .join("device/name");
        if !std::fs::read_to_string(sys_name)
            .is_ok_and(|name| name.to_ascii_lowercase().contains("haptic"))
        {
            continue;
        }
        let Ok(mut dev) = Device::open(std::path::Path::new("/dev/input").join(name)) else {
            continue;
        };
        if !dev
            .supported_ff()
            .is_some_and(|ff| ff.contains(FFEffectCode::FF_RUMBLE))
            || dev.set_nonblocking(true).is_err()
        {
            continue;
        }
        // The reference motor can start at zero gain. Some other motors have no gain control.
        let _ = dev.set_ff_gain(u16::MAX);
        if let Ok(effect) = dev.upload_ff_effect(FFEffectData {
            direction: 0,
            trigger: FFTrigger {
                button: 0,
                interval: 0,
            },
            replay: FFReplay {
                length: 14,
                delay: 0,
            },
            kind: FFEffectKind::Rumble {
                strong_magnitude: 0xB000,
                weak_magnitude: 0xB000,
            },
        }) {
            return Some(effect);
        }
    }
    None
}

pub struct Input {
    queue: Sender<(Action, Modifiers)>,
    haptic: Option<RefCell<FFEffect>>,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        let (queue, receiver) = async_channel::bounded(64);
        if let Err(err) = thread::Builder::new()
            .name("floatkb-input".into())
            .spawn(move || typing_worker(receiver))
        {
            eprintln!("floatkb: cannot start input worker: {err}");
        }
        Self {
            queue,
            haptic: haptic_effect().map(RefCell::new),
        }
    }

    /// Enqueues a keystroke without waiting for the compositor. Actions that
    /// only change the UI are ignored; taps are dropped if the queue is full.
    pub fn send(&self, action: Action, mods: Modifiers) {
        if matches!(action, Action::Char(_) | Action::Key(_) | Action::Space) {
            let _ = self.queue.try_send((action, mods));
        }
    }

    pub fn buzz(&self) {
        if let Some(effect) = &self.haptic {
            let _ = effect.borrow_mut().play(1);
        }
    }

    pub fn has_haptics(&self) -> bool {
        self.haptic.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_reference_rows_and_page_switches() {
        use Action::*;
        for (page, lengths, bottom_target) in [
            (super::Page::Abc, [9, 10, 9, 9, 5], super::Page::Sym),
            (super::Page::Sym, [9, 10, 10, 9, 5], super::Page::Abc),
            (super::Page::More, [9, 10, 10, 7, 5], super::Page::Abc),
        ] {
            let layout = rows(page);
            assert_eq!(layout.iter().map(Vec::len).collect::<Vec<_>>(), lengths);
            assert_eq!(
                layout[0].iter().map(|key| key.action).collect::<Vec<_>>(),
                [
                    Key("Escape"),
                    Key("Tab"),
                    Ctrl,
                    Alt,
                    Super,
                    Key("Left"),
                    Key("Down"),
                    Key("Up"),
                    Key("Right")
                ]
            );
            assert_eq!(layout[0][1].width, 1.25);
            assert_eq!(layout[4][0].action, Page(bottom_target));
            assert_eq!(layout[4][2].action, Space);
            assert_eq!(layout[4][2].width, 5.0);
            assert_eq!(layout[4][4].action, Key("Return"));
        }
        assert_eq!(rows(super::Page::Sym)[3][0].action, Page(super::Page::More));
        assert_eq!(rows(super::Page::More)[3][0].action, Page(super::Page::Sym));
        assert_eq!(rows(super::Page::More)[3][3].action, Key("Prior"));
    }

    #[test]
    fn hints_include_numbers_symbols_and_named_keys() {
        use Action::*;
        let abc = rows(super::Page::Abc);
        assert_eq!(abc[1][0].hint, Some(("1".into(), Char('1'))));
        assert_eq!(abc[2][0].hint, Some(("@".into(), Char('@'))));
        assert_eq!(abc[3][2].hint, Some(("\"".into(), Char('"'))));
        assert_eq!(abc[4][1].hint, Some(("⇥".into(), Key("Tab"))));
        assert_eq!(abc[4][3].hint, Some(("esc".into(), Key("Escape"))));
        let sym = rows(super::Page::Sym);
        assert_eq!(sym[1][9].hint, Some(("≠".into(), Char('≠'))));
        assert_eq!(sym[0][1].hint, None);
    }

    #[test]
    fn all_printable_ascii_has_a_us_keycode() {
        for ch in ' '..='~' {
            assert!(lookup(Action::Char(ch)).is_some(), "missing {ch:?}");
        }
        assert_eq!(lookup(Action::Char('a')), Some((KeyCode::KEY_A, false)));
        assert_eq!(lookup(Action::Char('A')), Some((KeyCode::KEY_A, true)));
        assert_eq!(lookup(Action::Char('?')), Some((KeyCode::KEY_SLASH, true)));
        assert_eq!(lookup(Action::Key("exclam")), Some((KeyCode::KEY_1, true)));
        assert_eq!(
            lookup(Action::Key("Next")),
            Some((KeyCode::KEY_PAGEDOWN, false))
        );
        assert_eq!(lookup(Action::Char('€')), None);
    }

    #[test]
    fn shifted_chord_releases_every_key_in_reverse_order() {
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        let mut events = Vec::new();
        tap(KeyCode::KEY_A, mods, true, |code, value| {
            events.push((code, value));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            events,
            [
                (KeyCode::KEY_LEFTSHIFT, 1),
                (KeyCode::KEY_LEFTCTRL, 1),
                (KeyCode::KEY_A, 1),
                (KeyCode::KEY_A, 0),
                (KeyCode::KEY_LEFTCTRL, 0),
                (KeyCode::KEY_LEFTSHIFT, 0),
            ]
        );
    }

    #[test]
    fn fallback_uses_keysyms_and_balanced_modifiers() {
        let mods = Modifiers {
            ctrl: true,
            logo: true,
            ..Modifiers::default()
        };
        assert_eq!(
            wtype_args(Action::Char('€'), mods).unwrap(),
            [
                "-M", "ctrl", "-M", "logo", "-k", "U20AC", "-m", "logo", "-m", "ctrl"
            ]
        );
        assert_eq!(
            wtype_args(Action::Char('-'), Modifiers::default()).unwrap(),
            ["-k", "minus"]
        );
        assert_eq!(
            wtype_args(Action::Key("BackSpace"), Modifiers::default()).unwrap(),
            ["-k", "BackSpace"]
        );
        assert!(wtype_args(Action::Page(Page::Abc), mods).is_none());
    }
}
