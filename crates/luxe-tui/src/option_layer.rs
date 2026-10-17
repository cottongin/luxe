//! Option layer — macOS Option+key characters read back as the Alt chords they were pressed as.
//!
//! On a Mac the Option key types characters: `⌥O` is `ø`, `⌥P` is `π`. A terminal can be told to
//! treat it as Alt instead, and Ghostty does that by default only for the "U.S." layouts. On the
//! others, "ABC" among them, Ghostty sends the typed character *with* the Alt flag set, so `⌥O`
//! arrives as `Alt+ø`: no binding names it, and the composer ignores an Alt chord, so the key does
//! nothing at all. Option combinations that type nothing (the accent keys `E`, `I`, `N`, `U`, `` ` ``)
//! arrive as plain `Alt+e` and so on, which is why some Alt chords worked and others did not.
//!
//! The Alt flag is what makes this safe to undo. A character typed from a layout that has it on a
//! key of its own (`ø` on a Norwegian keyboard) arrives without Alt and is left alone; only a
//! character that came with Alt held is read back to the key that made it. The table is the "ABC"
//! and "U.S." Option layer, which every Mac ships and which the two share.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// `key` with an Option-layer character read back to the key it was typed on, when it came with
/// Alt held; anything else unchanged.
pub(crate) fn normalize(key: KeyEvent) -> KeyEvent {
    let KeyCode::Char(typed) = key.code else { return key };
    if typed.is_ascii() || !key.modifiers.contains(KeyModifiers::ALT) {
        return key;
    }
    match base_key(typed) {
        Some((base, shifted)) => {
            let modifiers = match shifted {
                true => key.modifiers | KeyModifiers::SHIFT,
                false => key.modifiers,
            };
            KeyEvent { code: KeyCode::Char(base), modifiers, ..key }
        }
        None => key,
    }
}

/// The key an Option-layer character is typed on, and whether Shift was part of it.
fn base_key(typed: char) -> Option<(char, bool)> {
    let plain = match typed {
        'å' => 'a',
        '∫' => 'b',
        'ç' => 'c',
        '∂' => 'd',
        'ƒ' => 'f',
        '©' => 'g',
        '˙' => 'h',
        '∆' => 'j',
        '˚' => 'k',
        '¬' => 'l',
        'µ' => 'm',
        'ø' => 'o',
        'π' => 'p',
        'œ' => 'q',
        '®' => 'r',
        'ß' => 's',
        '†' => 't',
        '√' => 'v',
        '∑' => 'w',
        '≈' => 'x',
        '¥' => 'y',
        'Ω' => 'z',
        '¡' => '1',
        '™' => '2',
        '£' => '3',
        '¢' => '4',
        '∞' => '5',
        '§' => '6',
        '¶' => '7',
        '•' => '8',
        'ª' => '9',
        'º' => '0',
        '–' => '-',
        '≠' => '=',
        '“' => '[',
        '‘' => ']',
        '«' => '\\',
        '…' => ';',
        'æ' => '\'',
        '≤' => ',',
        '≥' => '.',
        '÷' => '/',
        _ => return shifted_base_key(typed).map(|base| (base, true)),
    };
    Some((plain, false))
}

/// The letter a Shift+Option character is typed on.
fn shifted_base_key(typed: char) -> Option<char> {
    Some(match typed {
        'Å' => 'a',
        'ı' => 'b',
        'Ç' => 'c',
        'Î' => 'd',
        '´' => 'e',
        'Ï' => 'f',
        '˝' => 'g',
        'Ó' => 'h',
        'ˆ' => 'i',
        'Ô' => 'j',
        '\u{f8ff}' => 'k',
        'Ò' => 'l',
        'Â' => 'm',
        '˜' => 'n',
        'Ø' => 'o',
        '∏' => 'p',
        'Œ' => 'q',
        '‰' => 'r',
        'Í' => 's',
        'ˇ' => 't',
        '¨' => 'u',
        '◊' => 'v',
        '„' => 'w',
        '˛' => 'x',
        'Á' => 'y',
        '¸' => 'z',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: char, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(code), modifiers)
    }

    /// `⌥O` from Ghostty on the "ABC" layout is the chord the preview advertises.
    #[test]
    fn an_option_character_held_with_alt_is_the_alt_chord() {
        assert_eq!(normalize(key('ø', KeyModifiers::ALT)), key('o', KeyModifiers::ALT));
        assert_eq!(normalize(key('π', KeyModifiers::ALT)), key('p', KeyModifiers::ALT));
        assert_eq!(normalize(key('∑', KeyModifiers::ALT)), key('w', KeyModifiers::ALT));
        assert_eq!(
            normalize(key('Ø', KeyModifiers::ALT)),
            key('o', KeyModifiers::ALT | KeyModifiers::SHIFT),
            "the Shift layer keeps its Shift"
        );
        assert_eq!(
            normalize(key('Ø', KeyModifiers::ALT | KeyModifiers::SHIFT)),
            key('o', KeyModifiers::ALT | KeyModifiers::SHIFT),
            "including when the terminal already said so"
        );
    }

    /// The whole Option layer of the "ABC" layout, as macOS's Keyboard Viewer shows it: every key
    /// that types a character reads back to that key. Written out here rather than derived, so a
    /// row lost from the table is a failure rather than a key that quietly stops working.
    #[test]
    fn every_option_key_on_the_abc_layout_reads_back() {
        let plain = "å a ∫ b ç c ∂ d ƒ f © g ˙ h ∆ j ˚ k ¬ l µ m ø o π p œ q ® r ß s † t √ v ∑ w \
                     ≈ x ¥ y Ω z ¡ 1 ™ 2 £ 3 ¢ 4 ∞ 5 § 6 ¶ 7 • 8 ª 9 º 0 – - ≠ = “ [ ‘ ] « \\ \
                     … ; æ ' ≤ , ≥ . ÷ /";
        let shifted = "Å a ı b Ç c Î d ´ e Ï f ˝ g Ó h ˆ i Ô j \u{f8ff} k Ò l Â m ˜ n Ø o ∏ p \
                       Œ q ‰ r Í s ˇ t ¨ u ◊ v „ w ˛ x Á y ¸ z";
        for (layer, modifiers) in
            [(plain, KeyModifiers::ALT), (shifted, KeyModifiers::ALT | KeyModifiers::SHIFT)]
        {
            let pairs: Vec<&str> = layer.split_whitespace().collect();
            for pair in pairs.chunks(2) {
                let typed = pair[0].chars().next().expect("a character");
                let base = pair[1].chars().next().expect("a key");
                assert_eq!(
                    normalize(key(typed, KeyModifiers::ALT)),
                    key(base, modifiers),
                    "{typed}"
                );
            }
        }
    }

    /// Typed without Alt it is a character someone meant, from a layout that has it on a key.
    #[test]
    fn a_character_typed_without_alt_is_left_alone() {
        assert_eq!(normalize(key('ø', KeyModifiers::NONE)), key('ø', KeyModifiers::NONE));
        assert_eq!(normalize(key('é', KeyModifiers::ALT)), key('é', KeyModifiers::ALT));
        assert_eq!(normalize(key('o', KeyModifiers::ALT)), key('o', KeyModifiers::ALT));
    }
}
