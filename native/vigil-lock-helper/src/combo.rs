//! Unlock-chord model: an **ordered sequence** of keys (modifiers and regular
//! keys, in press order) plus the two pure state machines that drive it:
//!
//! - [`CaptureAccumulator`] — records the chord as you press it (any number of
//!   keys, in order) and finalizes when you release the *anchor* (the first key
//!   you pressed), so you can hold the anchor and add keys freely until then.
//! - [`SequenceMatcher`] — during a freeze, fires `unlock` only when the chord's
//!   keys are pressed **in the recorded order** (foreign keys ignored, releasing
//!   a matched key resets progress).
//!
//! Both are platform-independent and exhaustively unit-tested; the macOS event
//! tap (`macos.rs`) only translates `CGEvent`s into `ChordKey` down/up + modifier
//! bitmasks and feeds them in. The canonical wire form is `+`-joined tokens in
//! press order (e.g. `ctrl+l+alt`) — order is significant, unlike a sorted
//! hotkey.

/// Carbon HIToolbox virtual key codes (`kVK_*`, `HIToolbox/Events.h`).
///
/// `objc2-core-graphics` does not re-export the named `KeyCode::ANSI_*`
/// constants the old `core-graphics` crate provided, so we carry the frozen
/// Carbon values directly. These are a stable macOS ABI and have not changed
/// since Carbon shipped.
#[cfg(target_os = "macos")]
mod keycode {
    pub const ANSI_A: u16 = 0x00;
    pub const ANSI_S: u16 = 0x01;
    pub const ANSI_D: u16 = 0x02;
    pub const ANSI_F: u16 = 0x03;
    pub const ANSI_H: u16 = 0x04;
    pub const ANSI_G: u16 = 0x05;
    pub const ANSI_Z: u16 = 0x06;
    pub const ANSI_X: u16 = 0x07;
    pub const ANSI_C: u16 = 0x08;
    pub const ANSI_V: u16 = 0x09;
    pub const ANSI_B: u16 = 0x0B;
    pub const ANSI_Q: u16 = 0x0C;
    pub const ANSI_W: u16 = 0x0D;
    pub const ANSI_E: u16 = 0x0E;
    pub const ANSI_R: u16 = 0x0F;
    pub const ANSI_Y: u16 = 0x10;
    pub const ANSI_T: u16 = 0x11;
    pub const ANSI_1: u16 = 0x12;
    pub const ANSI_2: u16 = 0x13;
    pub const ANSI_3: u16 = 0x14;
    pub const ANSI_4: u16 = 0x15;
    pub const ANSI_6: u16 = 0x16;
    pub const ANSI_5: u16 = 0x17;
    pub const ANSI_9: u16 = 0x19;
    pub const ANSI_7: u16 = 0x1A;
    pub const ANSI_8: u16 = 0x1C;
    pub const ANSI_0: u16 = 0x1D;
    pub const ANSI_O: u16 = 0x1F;
    pub const ANSI_U: u16 = 0x20;
    pub const ANSI_I: u16 = 0x22;
    pub const ANSI_P: u16 = 0x23;
    pub const ANSI_L: u16 = 0x25;
    pub const ANSI_J: u16 = 0x26;
    pub const ANSI_K: u16 = 0x28;
    pub const ANSI_N: u16 = 0x2D;
    pub const ANSI_M: u16 = 0x2E;
    pub const RETURN: u16 = 0x24;
    pub const TAB: u16 = 0x30;
    pub const SPACE: u16 = 0x31;
    pub const F1: u16 = 0x7A;
    pub const F2: u16 = 0x78;
    pub const F3: u16 = 0x63;
    pub const F4: u16 = 0x76;
    pub const F5: u16 = 0x60;
    pub const F6: u16 = 0x61;
    pub const F7: u16 = 0x62;
    pub const F8: u16 = 0x64;
    pub const F9: u16 = 0x65;
    pub const F10: u16 = 0x6D;
    pub const F11: u16 = 0x67;
    pub const F12: u16 = 0x6F;
}

// ── modifier identity + bitset ────────────────────────────────────────────────

/// Modifier bit values for the compact `u8` modifier set the capture/match state
/// machines pass around. Shared with `macos.rs`, which derives them from
/// `CGEventFlags`.
pub const MOD_CONTROL: u8 = 1;
pub const MOD_OPTION: u8 = 2;
pub const MOD_SHIFT: u8 = 4;
pub const MOD_COMMAND: u8 = 8;

/// The safety floor: a chord must contain at least this many keys. Below this an
/// unlock would be too easy to trigger by accident. (The old model required
/// ≥3 *modifiers* + a key; this is the looser "≥3 keys, any mix" rule.)
pub const MIN_CHORD_KEYS: usize = 3;

/// One of the four chord modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Control,
    Option,
    Shift,
    Command,
}

impl Modifier {
    /// Canonical token spelling (`ctrl`/`alt`/`shift`/`cmd`).
    pub fn canonical(self) -> &'static str {
        match self {
            Modifier::Control => "ctrl",
            Modifier::Option => "alt",
            Modifier::Shift => "shift",
            Modifier::Command => "cmd",
        }
    }

    /// This modifier's bit in the `MOD_*` set.
    pub fn bit(self) -> u8 {
        match self {
            Modifier::Control => MOD_CONTROL,
            Modifier::Option => MOD_OPTION,
            Modifier::Shift => MOD_SHIFT,
            Modifier::Command => MOD_COMMAND,
        }
    }
}

/// The four modifiers in their fixed bit order — used when expanding a modifier
/// bitmask diff into individual `ChordKey::Mod` events.
pub const MODIFIER_TABLE: [Modifier; 4] = [
    Modifier::Control,
    Modifier::Option,
    Modifier::Shift,
    Modifier::Command,
];

/// A single element of an ordered chord: a modifier or a regular key (keycode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKey {
    Mod(Modifier),
    Key(u16),
}

fn normalize_modifier_token(token: &str) -> Option<Modifier> {
    match token {
        "ctrl" | "control" => Some(Modifier::Control),
        "alt" | "option" | "opt" => Some(Modifier::Option),
        "shift" => Some(Modifier::Shift),
        "cmd" | "command" | "super" => Some(Modifier::Command),
        _ => None,
    }
}

fn keycode_for_key(token: &str) -> Option<u16> {
    #[cfg(not(target_os = "macos"))]
    let unsupported = None;
    #[cfg(target_os = "macos")]
    let unsupported = None;
    match token {
        #[cfg(target_os = "macos")]
        "a" => Some(keycode::ANSI_A),
        #[cfg(target_os = "macos")]
        "b" => Some(keycode::ANSI_B),
        #[cfg(target_os = "macos")]
        "c" => Some(keycode::ANSI_C),
        #[cfg(target_os = "macos")]
        "d" => Some(keycode::ANSI_D),
        #[cfg(target_os = "macos")]
        "e" => Some(keycode::ANSI_E),
        #[cfg(target_os = "macos")]
        "f" => Some(keycode::ANSI_F),
        #[cfg(target_os = "macos")]
        "g" => Some(keycode::ANSI_G),
        #[cfg(target_os = "macos")]
        "h" => Some(keycode::ANSI_H),
        #[cfg(target_os = "macos")]
        "i" => Some(keycode::ANSI_I),
        #[cfg(target_os = "macos")]
        "j" => Some(keycode::ANSI_J),
        #[cfg(target_os = "macos")]
        "k" => Some(keycode::ANSI_K),
        #[cfg(target_os = "macos")]
        "l" => Some(keycode::ANSI_L),
        #[cfg(target_os = "macos")]
        "m" => Some(keycode::ANSI_M),
        #[cfg(target_os = "macos")]
        "n" => Some(keycode::ANSI_N),
        #[cfg(target_os = "macos")]
        "o" => Some(keycode::ANSI_O),
        #[cfg(target_os = "macos")]
        "p" => Some(keycode::ANSI_P),
        #[cfg(target_os = "macos")]
        "q" => Some(keycode::ANSI_Q),
        #[cfg(target_os = "macos")]
        "r" => Some(keycode::ANSI_R),
        #[cfg(target_os = "macos")]
        "s" => Some(keycode::ANSI_S),
        #[cfg(target_os = "macos")]
        "t" => Some(keycode::ANSI_T),
        #[cfg(target_os = "macos")]
        "u" => Some(keycode::ANSI_U),
        #[cfg(target_os = "macos")]
        "v" => Some(keycode::ANSI_V),
        #[cfg(target_os = "macos")]
        "w" => Some(keycode::ANSI_W),
        #[cfg(target_os = "macos")]
        "x" => Some(keycode::ANSI_X),
        #[cfg(target_os = "macos")]
        "y" => Some(keycode::ANSI_Y),
        #[cfg(target_os = "macos")]
        "z" => Some(keycode::ANSI_Z),
        #[cfg(target_os = "macos")]
        "0" => Some(keycode::ANSI_0),
        #[cfg(target_os = "macos")]
        "1" => Some(keycode::ANSI_1),
        #[cfg(target_os = "macos")]
        "2" => Some(keycode::ANSI_2),
        #[cfg(target_os = "macos")]
        "3" => Some(keycode::ANSI_3),
        #[cfg(target_os = "macos")]
        "4" => Some(keycode::ANSI_4),
        #[cfg(target_os = "macos")]
        "5" => Some(keycode::ANSI_5),
        #[cfg(target_os = "macos")]
        "6" => Some(keycode::ANSI_6),
        #[cfg(target_os = "macos")]
        "7" => Some(keycode::ANSI_7),
        #[cfg(target_os = "macos")]
        "8" => Some(keycode::ANSI_8),
        #[cfg(target_os = "macos")]
        "9" => Some(keycode::ANSI_9),
        #[cfg(target_os = "macos")]
        "f1" => Some(keycode::F1),
        #[cfg(target_os = "macos")]
        "f2" => Some(keycode::F2),
        #[cfg(target_os = "macos")]
        "f3" => Some(keycode::F3),
        #[cfg(target_os = "macos")]
        "f4" => Some(keycode::F4),
        #[cfg(target_os = "macos")]
        "f5" => Some(keycode::F5),
        #[cfg(target_os = "macos")]
        "f6" => Some(keycode::F6),
        #[cfg(target_os = "macos")]
        "f7" => Some(keycode::F7),
        #[cfg(target_os = "macos")]
        "f8" => Some(keycode::F8),
        #[cfg(target_os = "macos")]
        "f9" => Some(keycode::F9),
        #[cfg(target_os = "macos")]
        "f10" => Some(keycode::F10),
        #[cfg(target_os = "macos")]
        "f11" => Some(keycode::F11),
        #[cfg(target_os = "macos")]
        "f12" => Some(keycode::F12),
        #[cfg(target_os = "macos")]
        "space" => Some(keycode::SPACE),
        #[cfg(target_os = "macos")]
        "tab" => Some(keycode::TAB),
        #[cfg(target_os = "macos")]
        "return" => Some(keycode::RETURN),
        _ => unsupported,
    }
}

/// Reverse of `keycode_for_key`: map a virtual keycode back to its canonical
/// token (e.g. `0x25` → `"l"`). `None` for an unmapped keycode (the same keys
/// `keycode_for_key` rejects, e.g. F13). The capture flow rejects a chord that
/// contains an unmappable key.
#[cfg(target_os = "macos")]
fn token_for_keycode(keycode: u16) -> Option<&'static str> {
    use keycode::*;
    let token = match keycode {
        ANSI_A => "a",
        ANSI_B => "b",
        ANSI_C => "c",
        ANSI_D => "d",
        ANSI_E => "e",
        ANSI_F => "f",
        ANSI_G => "g",
        ANSI_H => "h",
        ANSI_I => "i",
        ANSI_J => "j",
        ANSI_K => "k",
        ANSI_L => "l",
        ANSI_M => "m",
        ANSI_N => "n",
        ANSI_O => "o",
        ANSI_P => "p",
        ANSI_Q => "q",
        ANSI_R => "r",
        ANSI_S => "s",
        ANSI_T => "t",
        ANSI_U => "u",
        ANSI_V => "v",
        ANSI_W => "w",
        ANSI_X => "x",
        ANSI_Y => "y",
        ANSI_Z => "z",
        ANSI_0 => "0",
        ANSI_1 => "1",
        ANSI_2 => "2",
        ANSI_3 => "3",
        ANSI_4 => "4",
        ANSI_5 => "5",
        ANSI_6 => "6",
        ANSI_7 => "7",
        ANSI_8 => "8",
        ANSI_9 => "9",
        F1 => "f1",
        F2 => "f2",
        F3 => "f3",
        F4 => "f4",
        F5 => "f5",
        F6 => "f6",
        F7 => "f7",
        F8 => "f8",
        F9 => "f9",
        F10 => "f10",
        F11 => "f11",
        F12 => "f12",
        SPACE => "space",
        TAB => "tab",
        RETURN => "return",
        _ => return None,
    };
    Some(token)
}

/// Non-macOS stub: no keycodes map (the capture/freeze paths are macOS-only).
#[cfg(not(target_os = "macos"))]
fn token_for_keycode(_keycode: u16) -> Option<&'static str> {
    None
}

fn token_for_chordkey(key: ChordKey) -> Option<&'static str> {
    match key {
        ChordKey::Mod(m) => Some(m.canonical()),
        ChordKey::Key(kc) => token_for_keycode(kc),
    }
}

// ── parsing ───────────────────────────────────────────────────────────────────

/// A parsed, validated chord: the ordered keys plus its canonical wire form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// Keys in press order.
    pub keys: Vec<ChordKey>,
    /// `+`-joined canonical tokens, IN ORDER (e.g. `ctrl+l+alt`).
    pub canonical: String,
}

/// Parse a `+`-joined chord string, preserving token order. Rules:
/// - every token is a known modifier (incl. aliases) or a supported key;
/// - no `escape`;
/// - no duplicate key (you cannot hold the same key twice);
/// - at least [`MIN_CHORD_KEYS`] keys.
///
/// Order is significant and preserved: `ctrl+l+alt` ≠ `ctrl+alt+l`.
pub fn parse_chord(input: &str) -> Result<Chord, String> {
    let mut keys: Vec<ChordKey> = Vec::new();
    let mut tokens: Vec<String> = Vec::new();

    for token in input.split('+') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err("chord tokens cannot be empty".to_string());
        }
        let lowered = trimmed.to_ascii_lowercase();
        if lowered == "escape" {
            return Err("escape is not allowed in an unlock chord".to_string());
        }
        let (chord_key, canonical_token) = if let Some(m) = normalize_modifier_token(&lowered) {
            (ChordKey::Mod(m), m.canonical().to_string())
        } else if let Some(kc) = keycode_for_key(&lowered) {
            (ChordKey::Key(kc), lowered.clone())
        } else {
            return Err(format!("unsupported key: {lowered}"));
        };
        if keys.contains(&chord_key) {
            return Err(format!("duplicate key in chord: {lowered}"));
        }
        keys.push(chord_key);
        tokens.push(canonical_token);
    }

    if keys.len() < MIN_CHORD_KEYS {
        return Err(format!("chord must include at least {MIN_CHORD_KEYS} keys"));
    }

    Ok(Chord {
        keys,
        canonical: tokens.join("+"),
    })
}

/// Build the canonical wire form from a captured ordered sequence. `None` if any
/// element is an unmappable keycode (the chord cannot be registered).
pub fn canonical_from_sequence(keys: &[ChordKey]) -> Option<String> {
    let mut parts: Vec<&'static str> = Vec::with_capacity(keys.len());
    for &k in keys {
        parts.push(token_for_chordkey(k)?);
    }
    Some(parts.join("+"))
}

// ── capture: record a chord by pressing it (finalize on release) ──────────────

/// What the capture state machine wants the run loop to do after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureStep {
    /// Keep capturing.
    Continue,
    /// Esc — abort, record nothing.
    Cancel,
    /// The anchor was released — the ordered chord is finalized.
    Done(Vec<ChordKey>),
}

/// Accumulates a chord as the user presses it, in press order, and finalizes when
/// the user releases the **anchor** — the first key they pressed.
///
/// So you press an anchor (say `ctrl`), then press as many keys as you like, in
/// order, while holding it; releasing the anchor records the whole sequence.
/// Releasing a *non-anchor* key does NOT finish (let it go and keep building).
/// Esc cancels. Pure and fully testable; the macOS tap feeds it translated events.
pub struct CaptureAccumulator {
    seq: Vec<ChordKey>,
    /// The first key pressed; releasing it finalizes the chord.
    anchor: Option<ChordKey>,
    held_mods: u8,
    finalized: bool,
}

impl Default for CaptureAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureAccumulator {
    pub const fn new() -> Self {
        Self {
            seq: Vec::new(),
            anchor: None,
            held_mods: 0,
            finalized: false,
        }
    }

    /// Append a newly-pressed key in order (a held key cannot fire twice, so
    /// autorepeat is deduped); the FIRST key recorded becomes the anchor.
    fn push_key(&mut self, key: ChordKey) {
        if self.seq.contains(&key) {
            return;
        }
        self.seq.push(key);
        if self.anchor.is_none() {
            self.anchor = Some(key);
        }
    }

    /// A regular key went down. `is_escape` true → cancel. Otherwise the key is
    /// appended to the chord in press order.
    pub fn on_key_down(&mut self, keycode: u16, is_escape: bool) -> CaptureStep {
        if self.finalized {
            return CaptureStep::Continue;
        }
        if is_escape {
            return CaptureStep::Cancel;
        }
        self.push_key(ChordKey::Key(keycode));
        CaptureStep::Continue
    }

    /// A regular key was released → finalize ONLY if it is the anchor.
    pub fn on_key_up(&mut self, keycode: u16) -> CaptureStep {
        self.on_release(ChordKey::Key(keycode))
    }

    /// The modifier set changed to `now`. Newly-pressed modifiers are appended in
    /// press order; releasing the anchor modifier finalizes.
    pub fn on_flags(&mut self, now: u8) -> CaptureStep {
        if self.finalized {
            return CaptureStep::Continue;
        }
        let added = now & !self.held_mods;
        let removed = self.held_mods & !now;
        self.held_mods = now;
        for m in MODIFIER_TABLE {
            if added & m.bit() != 0 {
                self.push_key(ChordKey::Mod(m));
            }
        }
        for m in MODIFIER_TABLE {
            if removed & m.bit() != 0 {
                if let step @ CaptureStep::Done(_) = self.on_release(ChordKey::Mod(m)) {
                    return step;
                }
            }
        }
        CaptureStep::Continue
    }

    /// Finalize iff `key` is the anchor (and at least one key is recorded).
    fn on_release(&mut self, key: ChordKey) -> CaptureStep {
        if self.finalized || self.anchor != Some(key) || self.seq.is_empty() {
            return CaptureStep::Continue;
        }
        self.finalized = true;
        CaptureStep::Done(self.seq.clone())
    }
}

// ── match: fire unlock on the chord pressed IN ORDER ──────────────────────────

/// Detects the recorded chord pressed in **exact order**. Feed it `on_down` /
/// `on_up` for every key (modifiers and regular keys); `on_down` returns `true`
/// the instant the final key of the sequence is pressed in order.
///
/// Semantics:
/// - keys not in the chord are ignored (a stray keypress won't break an attempt);
/// - releasing a key that is part of the matched prefix resets progress (you must
///   hold the chord as you build it);
/// - an out-of-order chord key reseeds progress (so a fresh attempt can start
///   immediately);
/// - autorepeat (a second down for an already-held key) is ignored.
pub struct SequenceMatcher {
    target: Vec<ChordKey>,
    progress: usize,
    held: Vec<ChordKey>,
}

impl SequenceMatcher {
    pub fn new(target: Vec<ChordKey>) -> Self {
        Self {
            target,
            progress: 0,
            held: Vec::new(),
        }
    }

    /// Feed a key-down. Returns `true` iff the chord just completed in order.
    pub fn on_down(&mut self, key: ChordKey) -> bool {
        if self.held.contains(&key) {
            return false; // autorepeat / already held
        }
        self.held.push(key);

        let Some(pos) = self.target.iter().position(|&t| t == key) else {
            return false; // foreign key — ignore, don't disturb progress
        };

        if pos == self.progress {
            self.progress += 1;
        } else {
            // Out-of-order chord key: reseed. Targets are duplicate-free, so the
            // only key that can re-open an attempt is target[0].
            self.progress = usize::from(pos == 0);
        }

        if self.progress == self.target.len() {
            self.progress = 0; // ready for a fresh attempt next time
            return true;
        }
        false
    }

    /// Feed a key-up. Releasing a matched-prefix key resets progress.
    pub fn on_up(&mut self, key: ChordKey) {
        self.held.retain(|&h| h != key);
        if let Some(pos) = self.target.iter().position(|&t| t == key) {
            if pos < self.progress {
                self.progress = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_chord ──────────────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_preserves_press_order_and_normalizes() {
        let chord = parse_chord("CTRL + L + aLt").unwrap();
        assert_eq!(chord.canonical, "ctrl+l+alt");
        assert_eq!(
            chord.keys,
            vec![
                ChordKey::Mod(Modifier::Control),
                ChordKey::Key(keycode::ANSI_L),
                ChordKey::Mod(Modifier::Option),
            ]
        );
        // Order is significant: a different press order is a different chord.
        let other = parse_chord("ctrl+alt+l").unwrap();
        assert_eq!(other.canonical, "ctrl+alt+l");
        assert_ne!(chord.canonical, other.canonical);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_accepts_modifier_aliases_in_order() {
        let chord = parse_chord("control+option+super+space").unwrap();
        assert_eq!(chord.canonical, "ctrl+alt+cmd+space");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_rejects_too_short_dupes_escape_unknown() {
        // (label, input, exact Err message). Asserting the discriminator message
        // per row strictly increases coverage over a bare is_err() — it pins
        // WHICH guard fired, not just that one did.
        let rejects: &[(&str, &str, &str)] = &[
            // Below the floor (2 keys < MIN_CHORD_KEYS).
            (
                "two keys is below the floor",
                "ctrl+l",
                "chord must include at least 3 keys",
            ),
            // Duplicate modifier (a held key cannot fire twice).
            (
                "duplicate modifier rejected",
                "ctrl+l+ctrl",
                "duplicate key in chord: ctrl",
            ),
            // Duplicate regular key.
            (
                "duplicate key rejected",
                "ctrl+l+l",
                "duplicate key in chord: l",
            ),
            // Alias of an already-present modifier is still a duplicate.
            (
                "alias of present modifier is a duplicate",
                "ctrl+control+l",
                "duplicate key in chord: control",
            ),
            // Escape forbidden anywhere.
            (
                "escape forbidden anywhere",
                "ctrl+escape+l",
                "escape is not allowed in an unlock chord",
            ),
            // Unknown / unmapped key.
            (
                "unknown / unmapped key",
                "ctrl+alt+f13",
                "unsupported key: f13",
            ),
            // Empty token (double `+`).
            (
                "empty token rejected",
                "ctrl++l",
                "chord tokens cannot be empty",
            ),
        ];
        for (label, input, want_err) in rejects {
            let err =
                parse_chord(input).expect_err(&format!("{label}: {input:?} must be rejected"));
            assert_eq!(&err, want_err, "{label}: {input:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn canonical_from_sequence_round_trips_through_parse() {
        let seq = vec![
            ChordKey::Mod(Modifier::Control),
            ChordKey::Key(keycode::ANSI_L),
            ChordKey::Mod(Modifier::Option),
        ];
        let canon = canonical_from_sequence(&seq).unwrap();
        assert_eq!(canon, "ctrl+l+alt");
        assert_eq!(parse_chord(&canon).unwrap().keys, seq);
        // Unmappable keycode → None.
        assert_eq!(canonical_from_sequence(&[ChordKey::Key(0x6E)]), None);
    }

    // ── SequenceMatcher (pure, platform-independent) ─────────────────────────

    fn seq(keys: &[ChordKey]) -> SequenceMatcher {
        SequenceMatcher::new(keys.to_vec())
    }
    const A: ChordKey = ChordKey::Key(0x00);
    const B: ChordKey = ChordKey::Key(0x0B);
    const C: ChordKey = ChordKey::Key(0x08);
    const CTRL: ChordKey = ChordKey::Mod(Modifier::Control);
    const ALT: ChordKey = ChordKey::Mod(Modifier::Option);

    #[test]
    fn matcher_fires_on_exact_in_order_press() {
        let mut m = seq(&[CTRL, A, ALT]);
        assert!(!m.on_down(CTRL));
        assert!(!m.on_down(A));
        assert!(m.on_down(ALT), "completing in order fires unlock");
    }

    #[test]
    fn matcher_rejects_wrong_order() {
        let mut m = seq(&[CTRL, A, ALT]);
        assert!(!m.on_down(CTRL));
        assert!(!m.on_down(ALT)); // out of order → reseed to 0
        assert!(!m.on_down(A));
        // never completes in this attempt
        assert!(!m.on_down(ALT)); // ALT again but progress was reset
    }

    #[test]
    fn matcher_ignores_foreign_keys() {
        let mut m = seq(&[CTRL, A]);
        assert!(!m.on_down(CTRL));
        assert!(!m.on_down(B), "B is not in the chord → ignored");
        assert!(m.on_down(A), "foreign key did not disturb progress");
    }

    #[test]
    fn matcher_resets_when_prefix_key_released() {
        let mut m = seq(&[CTRL, A, ALT]);
        assert!(!m.on_down(CTRL));
        assert!(!m.on_down(A));
        m.on_up(CTRL); // released a matched-prefix key → reset
        assert!(!m.on_down(ALT), "must NOT unlock after releasing the hold");
    }

    #[test]
    fn matcher_dedupes_autorepeat() {
        let mut m = seq(&[A, B]);
        assert!(!m.on_down(A));
        assert!(!m.on_down(A), "autorepeat of a held key is ignored");
        assert!(m.on_down(B));
    }

    #[test]
    fn matcher_reseeds_for_a_fresh_attempt() {
        let mut m = seq(&[A, B]);
        assert!(!m.on_down(B)); // wrong start
        m.on_up(B);
        assert!(!m.on_down(A)); // fresh attempt
        assert!(m.on_down(B));
    }

    #[test]
    fn matcher_on_up_noop_when_pos_ge_progress_or_foreign() {
        // on_up resets progress ONLY when the released key is a matched-PREFIX key
        // (its target position `pos < progress`). Two non-reset branches:
        //
        //  (1) pos >= progress: release a chord key we have NOT consumed yet (its
        //      position is at/after the current progress). progress is untouched, so
        //      the in-order completion still fires.
        //  (2) foreign key: a key not in the target at all -> position() is None ->
        //      the `if let Some(pos)` is skipped -> progress untouched.
        //
        // Chord [CTRL, A, ALT]. Press CTRL (progress 1). Then:
        let mut m = seq(&[CTRL, A, ALT]);
        assert!(!m.on_down(CTRL)); // progress -> 1 (CTRL is target[0])

        // (1) Release ALT (target[2]); pos=2 >= progress=1 -> NOT < progress -> no
        //     reset. progress stays 1.
        m.on_up(ALT);
        // (2) Release B (foreign, not in target) -> position None -> no reset.
        m.on_up(B);

        // progress survived both releases: A then ALT still completes in order.
        assert!(!m.on_down(A), "A advances progress 1 -> 2");
        assert!(
            m.on_down(ALT),
            "completing in order fires; the two on_up calls did not reset progress"
        );
    }

    #[test]
    fn matcher_handles_a_stray_target_key_between() {
        // Chord [A, B, C]; pressing A, C (wrong, == target[2]) resets to 0
        // because C != target[1]; then A, B, C in order completes.
        let mut m = seq(&[A, B, C]);
        assert!(!m.on_down(A));
        assert!(!m.on_down(C)); // out of order → reseed (C != target[0]) → 0
        m.on_up(C);
        m.on_up(A);
        assert!(!m.on_down(A));
        assert!(!m.on_down(B));
        assert!(m.on_down(C));
    }

    // ── CaptureAccumulator (pure) ────────────────────────────────────────────

    #[test]
    fn capture_finalizes_only_on_anchor_release() {
        let mut acc = CaptureAccumulator::new();
        // ctrl is pressed first → it is the anchor. Then l, then alt, all held.
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue);
        assert_eq!(acc.on_key_down(0x25, false), CaptureStep::Continue); // l
        assert_eq!(
            acc.on_flags(MOD_CONTROL | MOD_OPTION),
            CaptureStep::Continue
        );
        // Releasing a NON-anchor key (drop alt) must NOT finalize.
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue);
        // Releasing the anchor (ctrl) finalizes the WHOLE press-order sequence
        // (alt stays recorded even though it was let go first).
        match acc.on_flags(0) {
            CaptureStep::Done(seq) => assert_eq!(
                seq,
                vec![
                    ChordKey::Mod(Modifier::Control),
                    ChordKey::Key(0x25),
                    ChordKey::Mod(Modifier::Option),
                ]
            ),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn capture_anchor_can_be_a_regular_key() {
        let mut acc = CaptureAccumulator::new();
        // l pressed first → anchor is the regular key l.
        assert_eq!(acc.on_key_down(0x25, false), CaptureStep::Continue);
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue);
        // Releasing the modifier (non-anchor) does not finalize.
        assert_eq!(acc.on_flags(0), CaptureStep::Continue);
        // Releasing l (the anchor) finalizes.
        match acc.on_key_up(0x25) {
            CaptureStep::Done(seq) => assert_eq!(
                seq,
                vec![ChordKey::Key(0x25), ChordKey::Mod(Modifier::Control)]
            ),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn capture_non_anchor_release_does_not_finalize() {
        let mut acc = CaptureAccumulator::new();
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue); // anchor = ctrl
        assert_eq!(acc.on_key_down(0x25, false), CaptureStep::Continue); // l
                                                                         // Releasing l (not the anchor) keeps capturing.
        assert_eq!(acc.on_key_up(0x25), CaptureStep::Continue);
    }

    #[test]
    fn capture_escape_cancels() {
        let mut acc = CaptureAccumulator::new();
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue);
        assert_eq!(acc.on_key_down(53, true), CaptureStep::Cancel);
    }

    #[test]
    fn capture_release_before_any_key_is_noop() {
        let mut acc = CaptureAccumulator::new();
        // A release with nothing recorded yet (no anchor) must NOT finalize.
        assert_eq!(acc.on_key_up(0x25), CaptureStep::Continue);
        assert_eq!(acc.on_flags(0), CaptureStep::Continue);
        // Re-asserting the same flags (no new bit) records nothing new.
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue);
        assert_eq!(acc.on_flags(MOD_CONTROL), CaptureStep::Continue);
    }
}
