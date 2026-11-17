//! Caption colors for the Windows shell. The Desktop Window Manager call lives in `main.rs` and
//! builds only on Windows. This library is the plan those calls follow, so it can be tested here.

/// Windows caption colors. macOS never calls into this.
pub mod caption {
    /// Page background #0a0b0d.
    const BG: (u8, u8, u8) = (0x0a, 0x0b, 0x0d);
    /// Console text #e9e9ea, so the caption stays readable on that background.
    const TEXT: (u8, u8, u8) = (0xe9, 0xe9, 0xea);
    /// DWMWA_BORDER_COLOR.
    pub const ATTR_BORDER: u32 = 34;
    /// DWMWA_CAPTION_COLOR.
    pub const ATTR_CAPTION: u32 = 35;
    /// DWMWA_TEXT_COLOR.
    pub const ATTR_TEXT_COLOR: u32 = 36;
    /// DWMWA_USE_IMMERSIVE_DARK_MODE on Windows 10 20H1 and Windows 11.
    pub const ATTR_IMMERSIVE: u32 = 20;
    /// The same flag on Windows 10 builds that predate attribute 20.
    pub const ATTR_IMMERSIVE_OLD: u32 = 19;

    /// COLORREF is 0x00BBGGRR.
    pub fn color_ref(rgb: (u8, u8, u8)) -> u32 {
        (rgb.2 as u32) << 16 | (rgb.1 as u32) << 8 | rgb.0 as u32
    }

    pub fn bg_ref() -> u32 {
        color_ref(BG)
    }

    pub fn text_ref() -> u32 {
        color_ref(TEXT)
    }

    /// A DWM call succeeded. A negative code means Windows refused the attribute.
    pub fn dwm_accepted(code: i32) -> bool {
        code >= 0
    }

    /// Calls to make, in order. `immersive_ok` means attribute 20 was accepted, so the older one is skipped.
    /// Caption, text and border are Windows 11. A refusal there still leaves the dark caption from 20 or 19.
    pub fn attempts(immersive_ok: bool) -> Vec<(u32, u32)> {
        let mut steps = vec![(ATTR_IMMERSIVE, 1)];
        if !immersive_ok {
            steps.push((ATTR_IMMERSIVE_OLD, 1));
        }
        steps.push((ATTR_CAPTION, bg_ref()));
        steps.push((ATTR_TEXT_COLOR, text_ref()));
        steps.push((ATTR_BORDER, bg_ref()));
        steps
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn colors_match_the_console() {
            assert_eq!(bg_ref(), 0x000d_0b0a);
            assert_eq!(text_ref(), 0x00ea_e9e9);
        }

        #[test]
        fn windows_11_paints_caption_text_and_border() {
            let steps = attempts(true);
            assert_eq!(steps[0], (ATTR_IMMERSIVE, 1));
            assert!(steps.contains(&(ATTR_CAPTION, bg_ref())));
            assert!(steps.contains(&(ATTR_TEXT_COLOR, text_ref())));
            assert!(steps.contains(&(ATTR_BORDER, bg_ref())));
            assert!(!steps.iter().any(|(attr, _)| *attr == ATTR_IMMERSIVE_OLD));
        }

        #[test]
        fn older_windows_still_asks_for_the_page_color() {
            let steps = attempts(false);
            assert_eq!(steps[1], (ATTR_IMMERSIVE_OLD, 1));
            assert!(steps.contains(&(ATTR_CAPTION, bg_ref())));
            assert!(steps.contains(&(ATTR_TEXT_COLOR, text_ref())));
            assert!(steps.contains(&(ATTR_BORDER, bg_ref())));
        }

        #[test]
        fn a_refused_call_is_not_success() {
            assert!(dwm_accepted(0));
            assert!(!dwm_accepted(-2_147_024_809));
        }
    }
}
