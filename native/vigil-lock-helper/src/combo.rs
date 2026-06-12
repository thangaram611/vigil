#[cfg(target_os = "macos")]
use core_graphics::event::{CGEventFlags, KeyCode};

#[cfg(not(target_os = "macos"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CGEventFlags(u8);

#[cfg(not(target_os = "macos"))]
impl core::ops::BitOr for CGEventFlags {
    type Output = Self;
    fn bitor(self, _rhs: Self) -> Self::Output {
        self
    }
}

#[cfg(not(target_os = "macos"))]
impl CGEventFlags {
    pub const CGEventFlagControl: Self = Self(0);
    pub const CGEventFlagShift: Self = Self(0);
    pub const CGEventFlagAlternate: Self = Self(0);
    pub const CGEventFlagCommand: Self = Self(0);
    pub const fn contains(self, _other: Self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredFlags {
    pub control: bool,
    pub option: bool,
    pub shift: bool,
    pub command: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub required_flags: RequiredFlags,
    pub final_keycode: u16,
    pub canonical: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModifierToken {
    Control,
    Option,
    Shift,
    Command,
}

impl ModifierToken {
    fn canonical(self) -> &'static str {
        match self {
            ModifierToken::Control => "ctrl",
            ModifierToken::Option => "alt",
            ModifierToken::Shift => "shift",
            ModifierToken::Command => "cmd",
        }
    }
}

fn normalize_modifier_token(token: &str) -> Option<ModifierToken> {
    match token {
        "ctrl" | "control" => Some(ModifierToken::Control),
        "alt" | "option" | "opt" => Some(ModifierToken::Option),
        "shift" => Some(ModifierToken::Shift),
        "cmd" | "command" | "super" => Some(ModifierToken::Command),
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
        "a" => Some(KeyCode::ANSI_A),
        #[cfg(target_os = "macos")]
        "b" => Some(KeyCode::ANSI_B),
        #[cfg(target_os = "macos")]
        "c" => Some(KeyCode::ANSI_C),
        #[cfg(target_os = "macos")]
        "d" => Some(KeyCode::ANSI_D),
        #[cfg(target_os = "macos")]
        "e" => Some(KeyCode::ANSI_E),
        #[cfg(target_os = "macos")]
        "f" => Some(KeyCode::ANSI_F),
        #[cfg(target_os = "macos")]
        "g" => Some(KeyCode::ANSI_G),
        #[cfg(target_os = "macos")]
        "h" => Some(KeyCode::ANSI_H),
        #[cfg(target_os = "macos")]
        "i" => Some(KeyCode::ANSI_I),
        #[cfg(target_os = "macos")]
        "j" => Some(KeyCode::ANSI_J),
        #[cfg(target_os = "macos")]
        "k" => Some(KeyCode::ANSI_K),
        #[cfg(target_os = "macos")]
        "l" => Some(KeyCode::ANSI_L),
        #[cfg(target_os = "macos")]
        "m" => Some(KeyCode::ANSI_M),
        #[cfg(target_os = "macos")]
        "n" => Some(KeyCode::ANSI_N),
        #[cfg(target_os = "macos")]
        "o" => Some(KeyCode::ANSI_O),
        #[cfg(target_os = "macos")]
        "p" => Some(KeyCode::ANSI_P),
        #[cfg(target_os = "macos")]
        "q" => Some(KeyCode::ANSI_Q),
        #[cfg(target_os = "macos")]
        "r" => Some(KeyCode::ANSI_R),
        #[cfg(target_os = "macos")]
        "s" => Some(KeyCode::ANSI_S),
        #[cfg(target_os = "macos")]
        "t" => Some(KeyCode::ANSI_T),
        #[cfg(target_os = "macos")]
        "u" => Some(KeyCode::ANSI_U),
        #[cfg(target_os = "macos")]
        "v" => Some(KeyCode::ANSI_V),
        #[cfg(target_os = "macos")]
        "w" => Some(KeyCode::ANSI_W),
        #[cfg(target_os = "macos")]
        "x" => Some(KeyCode::ANSI_X),
        #[cfg(target_os = "macos")]
        "y" => Some(KeyCode::ANSI_Y),
        #[cfg(target_os = "macos")]
        "z" => Some(KeyCode::ANSI_Z),
        #[cfg(target_os = "macos")]
        "0" => Some(KeyCode::ANSI_0),
        #[cfg(target_os = "macos")]
        "1" => Some(KeyCode::ANSI_1),
        #[cfg(target_os = "macos")]
        "2" => Some(KeyCode::ANSI_2),
        #[cfg(target_os = "macos")]
        "3" => Some(KeyCode::ANSI_3),
        #[cfg(target_os = "macos")]
        "4" => Some(KeyCode::ANSI_4),
        #[cfg(target_os = "macos")]
        "5" => Some(KeyCode::ANSI_5),
        #[cfg(target_os = "macos")]
        "6" => Some(KeyCode::ANSI_6),
        #[cfg(target_os = "macos")]
        "7" => Some(KeyCode::ANSI_7),
        #[cfg(target_os = "macos")]
        "8" => Some(KeyCode::ANSI_8),
        #[cfg(target_os = "macos")]
        "9" => Some(KeyCode::ANSI_9),
        #[cfg(target_os = "macos")]
        "f1" => Some(KeyCode::F1),
        #[cfg(target_os = "macos")]
        "f2" => Some(KeyCode::F2),
        #[cfg(target_os = "macos")]
        "f3" => Some(KeyCode::F3),
        #[cfg(target_os = "macos")]
        "f4" => Some(KeyCode::F4),
        #[cfg(target_os = "macos")]
        "f5" => Some(KeyCode::F5),
        #[cfg(target_os = "macos")]
        "f6" => Some(KeyCode::F6),
        #[cfg(target_os = "macos")]
        "f7" => Some(KeyCode::F7),
        #[cfg(target_os = "macos")]
        "f8" => Some(KeyCode::F8),
        #[cfg(target_os = "macos")]
        "f9" => Some(KeyCode::F9),
        #[cfg(target_os = "macos")]
        "f10" => Some(KeyCode::F10),
        #[cfg(target_os = "macos")]
        "f11" => Some(KeyCode::F11),
        #[cfg(target_os = "macos")]
        "f12" => Some(KeyCode::F12),
        #[cfg(target_os = "macos")]
        "space" => Some(KeyCode::SPACE),
        #[cfg(target_os = "macos")]
        "tab" => Some(KeyCode::TAB),
        #[cfg(target_os = "macos")]
        "return" => Some(KeyCode::RETURN),
        _ => unsupported,
    }
}

pub fn parse_combo(input: &str) -> Result<Combo, String> {
    let raw_tokens: Vec<&str> = input.split('+').collect();
    if raw_tokens.is_empty() {
        return Err("combo must contain at least one modifier and one final key".to_string());
    }

    let mut modifiers: RequiredFlags = RequiredFlags {
        control: false,
        option: false,
        shift: false,
        command: false,
    };
    let mut seen_mod = std::collections::HashSet::new();
    let mut final_key: Option<String> = None;

    for token in raw_tokens {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err("combo tokens cannot be empty".to_string());
        }
        let lowered = trimmed.to_ascii_lowercase();
        let is_modifier = normalize_modifier_token(&lowered);
        let Some(modifier) = is_modifier else {
            if lowered == "escape" {
                return Err("escape is not allowed as the unlock key".to_string());
            }
            if final_key.is_some() {
                return Err("combo cannot contain a second non-modifier key".to_string());
            }
            final_key = Some(lowered);
            continue;
        };
        let canonical_modifier = modifier.canonical();
        if !seen_mod.insert(canonical_modifier.to_string()) {
            return Err(format!("duplicate modifier token: {canonical_modifier}"));
        }
        match modifier {
            ModifierToken::Control => modifiers.control = true,
            ModifierToken::Option => modifiers.option = true,
            ModifierToken::Shift => modifiers.shift = true,
            ModifierToken::Command => modifiers.command = true,
        }
    }

    let final_key = final_key.ok_or_else(|| "combo must include a non-modifier key".to_string())?;
    let num_modifiers = (modifiers.control as u8)
        + (modifiers.option as u8)
        + (modifiers.shift as u8)
        + (modifiers.command as u8);
    if num_modifiers < 3 {
        return Err("combo must include at least three modifiers".to_string());
    }

    let final_keycode =
        keycode_for_key(&final_key).ok_or_else(|| format!("unsupported final key: {final_key}"))?;

    let mut canonical_parts = Vec::new();
    if modifiers.control {
        canonical_parts.push("ctrl");
    }
    if modifiers.option {
        canonical_parts.push("alt");
    }
    if modifiers.shift {
        canonical_parts.push("shift");
    }
    if modifiers.command {
        canonical_parts.push("cmd");
    }
    canonical_parts.push(&final_key);

    Ok(Combo {
        required_flags: modifiers,
        final_keycode,
        canonical: canonical_parts.join("+"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aliases_and_canonicalizes() {
        let combo = parse_combo("CMD + aLt + control + shift + L").unwrap();
        assert!(combo.required_flags.control);
        assert!(combo.required_flags.option);
        assert!(combo.required_flags.shift);
        assert!(combo.required_flags.command);
        assert_eq!(combo.final_keycode, KeyCode::ANSI_L);
        assert_eq!(combo.canonical, "ctrl+alt+shift+cmd+l");
    }

    #[test]
    fn parse_requires_three_modifiers_and_final_key() {
        assert!(
            parse_combo("ctrl+alt+L").is_err(),
            "at least three modifiers required"
        );
        assert!(
            parse_combo("ctrl+alt+shift+cmd").is_err(),
            "must include a final key"
        );
        assert!(
            parse_combo("ctrl+alt+shift+cmd+escape").is_err(),
            "escape forbidden"
        );
    }

    #[test]
    fn parse_rejects_duplicate_and_unknown() {
        assert!(
            parse_combo("ctrl+alt+shift+cmd+ctrl+l").is_err(),
            "duplicate modifier not allowed"
        );
        assert!(
            parse_combo("ctrl+alt+opt+shift+cmd+l").is_err(),
            "duplicate modifier aliases not allowed"
        );
        assert!(
            parse_combo("ctrl+alt+shift+cmd+f13").is_err(),
            "unknown key rejected"
        );
    }

    #[test]
    fn event_matches_combo_checks_required_state() {
        let combo = parse_combo("ctrl+alt+shift+cmd+l").unwrap();
        let missing_shift = CGEventFlags::CGEventFlagControl
            | CGEventFlags::CGEventFlagAlternate
            | CGEventFlags::CGEventFlagCommand;
        assert!(!event_matches_combo(
            &combo,
            combo.final_keycode,
            missing_shift
        ));
        let exact = CGEventFlags::CGEventFlagControl
            | CGEventFlags::CGEventFlagAlternate
            | CGEventFlags::CGEventFlagShift
            | CGEventFlags::CGEventFlagCommand;
        assert!(event_matches_combo(&combo, combo.final_keycode, exact));
        assert!(!event_matches_combo(&combo, combo.final_keycode + 1, exact));
    }
}

#[allow(dead_code)]
pub fn event_matches_combo(combo: &Combo, keycode: u16, flags: CGEventFlags) -> bool {
    if keycode != combo.final_keycode {
        return false;
    }
    let has_control = flags.contains(CGEventFlags::CGEventFlagControl);
    let has_shift = flags.contains(CGEventFlags::CGEventFlagShift);
    let has_option = flags.contains(CGEventFlags::CGEventFlagAlternate);
    let has_command = flags.contains(CGEventFlags::CGEventFlagCommand);

    if combo.required_flags.control && !has_control {
        return false;
    }
    if combo.required_flags.option && !has_option {
        return false;
    }
    if combo.required_flags.shift && !has_shift {
        return false;
    }
    if combo.required_flags.command && !has_command {
        return false;
    }
    true
}
